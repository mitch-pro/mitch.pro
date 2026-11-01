import RFB from '/novnc/core/rfb.js';

const $ = id => document.getElementById(id);
const query = new URLSearchParams(location.search);
const id = query.get('id') || '';
// `?container=owner` opens the owner's private container through the same page.
// Its API lives under /api/admin/owner-container and needs the admin passphrase.
const ownerMode = query.get('container') === 'owner';
const OWNER_API = '/api/admin/owner-container';
if (ownerMode) {
  const back = document.querySelector('.back-button');
  if (back) { back.href = '/admin/'; back.setAttribute('aria-label', 'Back to admin'); }
}
const screen = $('screen'), frame = $('display-frame'), cover = $('connection-cover');
const headers = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
const MAX_RETRIES = 3;
let rfb = null, generation = 0, controller = null, reconnectTimer = null, connectTimer = null;
let resumeWhenVisible = false, reconnectAttempts = 0, connected = false, connecting = false, disposed = false, powerBusy = false;
let leaseState = { remainingSeconds: 21600, maxUptimeSeconds: 21600, canExtend: true, extended: false, isExempt: false };
let hasWarned10m = false, hasWarned3m = false;
let uptimeInterval = null;
const uptimePillText = $('uptime-pill-text'), uptimeBtn = $('uptime-button');
const uptimeDialog = $('uptime-dialog');

// Owner container info, shaped like /api/vm/computers/<id> so connect() is shared.
// The owner has no session limit, so the lease is always exempt.
async function ownerInfo(signal) {
  const d = await request(OWNER_API, { signal });
  if (!d.exists) throw new Error('This computer could not be found.');
  return { computer: { name: d.name || 'Container', status: d.status, lease: { remainingSeconds: 0, maxUptimeSeconds: 0, canExtend: false, extended: false, isExempt: true } } };
}

function formatUptime(seconds) {
  const total = Math.max(0, Math.floor(Number(seconds) || 0));
  const m = Math.floor(total / 60);
  const s = total % 60;
  if (m >= 60) {
    const h = Math.floor(m / 60);
    const remM = m % 60;
    return `${h}h ${remM}m`;
  }
  return `${m}:${s < 10 ? '0' : ''}${s}`;
}

function updateUptimeDisplay() {
  if (!uptimePillText || !uptimeBtn) return;
  if (leaseState.isExempt) {
    uptimePillText.textContent = 'Unlimited';
    uptimeBtn.classList.remove('warning', 'urgent');
    uptimeBtn.classList.add('normal');
    uptimeBtn.title = 'Admin session: unlimited time';
    const dialogTime = $('uptime-dialog-time');
    if (dialogTime) dialogTime.textContent = 'Unlimited';
    const extendBox = $('uptime-extend-box'), extendedBox = $('uptime-extended-box');
    if (extendBox) extendBox.hidden = true;
    if (extendedBox) extendedBox.hidden = true;
    const dialogTitle = $('uptime-dialog-title'), dialogDesc = $('uptime-dialog-desc');
    if (dialogTitle && dialogDesc) {
      dialogTitle.textContent = 'Desktop Session Time';
      dialogDesc.innerHTML = 'You are an administrator. Your computer has <strong>Unlimited</strong> session time with no daily limit.';
    }
    return;
  }
  const rem = leaseState.remainingSeconds;
  uptimePillText.textContent = formatUptime(rem);
  const dialogTime = $('uptime-dialog-time');
  if (dialogTime) dialogTime.textContent = formatUptime(rem);

  uptimeBtn.classList.remove('normal', 'warning', 'urgent');
  if (rem <= 300) {
    uptimeBtn.classList.add('urgent');
  } else if (rem <= 900) {
    uptimeBtn.classList.add('warning');
  } else {
    uptimeBtn.classList.add('normal');
  }

  const canExtend = leaseState.canExtend && !leaseState.extended;
  const extendBox = $('uptime-extend-box'), extendedBox = $('uptime-extended-box');
  if (extendBox) extendBox.hidden = !canExtend;
  if (extendedBox) extendedBox.hidden = canExtend;

  const dialogTitle = $('uptime-dialog-title'), dialogDesc = $('uptime-dialog-desc');
  if (dialogTitle && dialogDesc) {
    if (canExtend) {
      dialogTitle.textContent = rem <= 600 ? 'Session Expiring Soon' : 'Desktop Session Time';
      dialogDesc.innerHTML = `Your desktop will automatically shut down in <strong id="uptime-dialog-time">${formatUptime(rem)}</strong> (maximum daily limit is 6 hours).`;
    } else {
      dialogTitle.textContent = rem <= 600 ? 'Final Warning: Session Ending' : 'Desktop Session Time';
      dialogDesc.innerHTML = `Your desktop will automatically shut down in <strong id="uptime-dialog-time">${formatUptime(rem)}</strong> (maximum daily limit is 6 hours).`;
    }
  }
}

function checkUptimeAlerts() {
  if (disposed || !connected || leaseState.isExempt) return;
  const rem = leaseState.remainingSeconds;
  if (rem <= 0) {
    closeConnection();
    connectionState('Daily Limit Reached', 'disconnected');
    showCover('Daily Limit Reached (6 hours)', 'Your computer was shut down because you have reached your maximum daily limit of 6 hours. You can start it again tomorrow.', false, false);
    return;
  }
  if (rem <= 600 && !hasWarned10m) {
    hasWarned10m = true;
    openUptimeModal();
  } else if (rem <= 180 && !hasWarned3m) {
    hasWarned3m = true;
    openUptimeModal();
  }
}

function openUptimeModal() {
  if (uptimeDialog && !uptimeDialog.open) {
    uptimeDialog.showModal();
  }
}

async function extendSession() {
  const extendBtn = $('uptime-extend-btn');
  if (extendBtn) extendBtn.disabled = true;
  try {
    const data = await request(`/api/vm/computers/${encodeURIComponent(id)}/extend`, { method: 'POST', headers, body: '{}' });
    if (data.lease) {
      leaseState = {
        remainingSeconds: data.lease.remainingSeconds,
        maxUptimeSeconds: data.lease.maxUptimeSeconds,
        canExtend: Boolean(data.lease.canExtend),
        extended: Boolean(data.lease.extended),
        isExempt: Boolean(data.lease.isExempt),
      };
      hasWarned10m = !leaseState.isExempt && leaseState.remainingSeconds <= 600;
      hasWarned3m = false;
      updateUptimeDisplay();
      uptimeDialog.close();
      showNotice(data.message || 'Session extended by 30 minutes.');
    }
  } catch (error) {
    alert(error.message || 'Could not extend session.');
  } finally {
    if (extendBtn) extendBtn.disabled = false;
  }
}

async function syncLease() {
  if (disposed || !id || ownerMode) return;
  try {
    const data = await request(`/api/vm/computers/${encodeURIComponent(id)}`);
    if (data?.computer?.lease) {
      leaseState = {
        remainingSeconds: data.computer.lease.remainingSeconds,
        maxUptimeSeconds: data.computer.lease.maxUptimeSeconds,
        canExtend: Boolean(data.computer.lease.canExtend),
        extended: Boolean(data.computer.lease.extended),
        isExempt: Boolean(data.computer.lease.isExempt),
      };
      updateUptimeDisplay();
      checkUptimeAlerts();
    }
  } catch {}
}

function connectionState(label, type = '') {
  $('machine-state').className = type;
  $('machine-state').replaceChildren(Object.assign(document.createElement('i'), { ariaHidden: 'true' }), document.createTextNode(' ' + label));
  document.querySelectorAll('[data-connected]').forEach(button => { button.disabled = !connected; });
}
function showCover(heading, message, canRetry = false, busy = false) {
  cover.classList.remove('is-hidden');
  $('connection-title').textContent = heading; $('connection-copy').textContent = message;
  $('retry-button').hidden = !canRetry; $('connection-spinner').hidden = !busy;
}
function closeConnection() {
  generation++; controller?.abort(); controller = null;
  clearTimeout(reconnectTimer); clearTimeout(connectTimer);
  clearInterval(uptimeInterval); uptimeInterval = null;
  const old = rfb; rfb = null; connected = false; connecting = false;
  if (old) { try { old.disconnect(); } catch {} }
  hideLocalCursor();
}
function friendlyError(status, data) {
  if (status === 401) return 'Your session expired. Go back to My Computer to sign in again.';
  if (status === 403) return 'You do not have permission to access this computer.';
  if (status === 404) return 'This computer could not be found.';
  if (status === 429) return 'Please wait a minute before opening another desktop connection.';
  return data?.error || 'Your computer could not be reached.';
}
function adminHeaders(extra = {}) {
  const h = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1', ...extra };
  const pass = sessionStorage.getItem('admin_passphrase');
  if (pass) h['X-Admin-Passphrase'] = pass;
  return h;
}
// Asks for the admin passphrase in a dialog on this page and verifies it
// server-side; resolves once it is stored for later requests.
function askPassphrase() {
  const dialog = $('passphrase-dialog'), form = $('passphrase-form'), input = $('passphrase-input');
  const status = $('passphrase-status'), submit = $('passphrase-submit'), cancel = $('passphrase-cancel');
  return new Promise((resolve, reject) => {
    status.textContent = ''; input.value = '';
    const done = (fn, value) => { form.onsubmit = null; cancel.onclick = null; dialog.close(); fn(value); };
    cancel.onclick = () => done(reject, new Error('Admin passphrase required.'));
    form.onsubmit = async event => {
      event.preventDefault();
      const pass = input.value.trim();
      if (!pass) return;
      submit.disabled = true; status.textContent = 'Verifying...';
      try {
        const res = await fetch('/api/admin/passphrase-status', {
          method: 'POST', credentials: 'same-origin', cache: 'no-store',
          headers: adminHeaders({ 'X-Admin-Passphrase': pass }), body: JSON.stringify({ passphrase: pass }),
        });
        const d = await res.json().catch(() => ({}));
        if (res.ok && d.ok) { sessionStorage.setItem('admin_passphrase', pass); done(resolve, pass); return; }
        sessionStorage.removeItem('admin_passphrase');
        status.textContent = 'Incorrect admin passphrase. Try again.'; input.select();
      } catch { status.textContent = 'Verification failed. Try again.'; }
      finally { submit.disabled = false; }
    };
    dialog.showModal(); input.focus();
  });
}
async function request(url, options = {}) {
  const send = () => fetch(url, { credentials: 'same-origin', cache: 'no-store', ...options, ...(ownerMode ? { headers: adminHeaders(options.headers || {}) } : {}) });
  let response = await send();
  let data = await response.json().catch(() => ({}));
  if (ownerMode && response.status === 403 && data.error === 'invalid_passphrase') {
    sessionStorage.removeItem('admin_passphrase');
    await askPassphrase();
    response = await send();
    data = await response.json().catch(() => ({}));
  }
  if (ownerMode && response.status === 403 && data.error === 'passphrase_not_configured') {
    throw new Error('Set your admin passphrase on the admin page first.');
  }
  if (!response.ok) { const error = new Error(friendlyError(response.status, data)); error.status = response.status; error.code = data.code; throw error; }
  return data;
}
function interrupted(token, clean = false) {
  if (disposed || token !== generation) return;
  closeConnection(); connectionState('Disconnected', 'disconnected');
  // A connection the server ends on its own (about every five minutes) is
  // reconnected like any other drop; closing it from this page (power actions,
  // leaving) bumps `generation` first, so those never reach here.
  if (document.hidden) {
    // Reconnect when the tab is visible again (see visibilitychange below).
    resumeWhenVisible = true;
    showCover('Desktop disconnected', 'Reconnecting when you come back to this tab.', false, true);
    return;
  }
  if (reconnectAttempts >= MAX_RETRIES) {
    showCover('Desktop disconnected', clean ? 'The remote desktop connection ended.' : 'Check your connection, then reconnect to your computer.', true);
    return;
  }
  reconnectAttempts++;
  showCover('Reconnecting to your desktop', `Connection interrupted. Trying again (${reconnectAttempts} of ${MAX_RETRIES})...`, false, true);
  reconnectTimer = setTimeout(connect, 1500 * reconnectAttempts);
}
async function connect() {
  if (disposed || powerBusy) return;
  closeConnection(); const token = generation;
  controller = new AbortController(); const signal = controller.signal; connecting = true;
  showCover(reconnectAttempts ? 'Reconnecting to your desktop' : 'Opening your desktop', 'Connecting securely...', false, true);
  connectionState('Connecting');
  try {
    if (!ownerMode && !/^[a-zA-Z0-9_-]{1,80}$/.test(id)) throw new Error('Go back to My Computer and choose a desktop.');
    const data = ownerMode ? await ownerInfo(signal) : await request(`/api/vm/computers/${encodeURIComponent(id)}`, { signal });
    if (token !== generation) return;
    const computer = data.computer;
    $('machine-name').textContent = computer?.name || 'My Computer';
    document.title = `${computer?.name || 'My Computer'} - ${location.hostname}`;
    if (computer?.lease) {
      leaseState = {
        remainingSeconds: computer.lease.remainingSeconds,
        maxUptimeSeconds: computer.lease.maxUptimeSeconds,
        canExtend: Boolean(computer.lease.canExtend),
        extended: Boolean(computer.lease.extended),
        isExempt: Boolean(computer.lease.isExempt),
      };
      updateUptimeDisplay();
    }
    if (computer?.desktopSetup?.status === 'failed') showNotice('Desktop repair needs another try. You can still open the console; choose Repair desktop in My Computer to retry.');
    if (computer?.status !== 'running') throw new Error('Your computer is offline. Start it from My Computer, then reconnect.');
    let session;
    const setupDeadline = Date.now() + 8 * 60 * 1000;
    while (!session) {
      try {
        session = ownerMode
          ? await request(`${OWNER_API}/console`, {method:'POST',headers,body:JSON.stringify({mode:'desktop'}),signal})
          : await request(`/api/vm/computers/${encodeURIComponent(id)}/desktop-session`, {method:'POST',headers,body:'{}',signal});
      } catch (error) {
        if (error.code !== 'desktop_preparing' || Date.now() > setupDeadline) throw error;
        showCover('Preparing your desktop', 'Setting up automatic sign-in and video support. This can take a few minutes on the first start.', false, true);
        await new Promise((resolve,reject) => {
          const onAbort=()=>{clearTimeout(timer);reject(new DOMException('Cancelled','AbortError'));};
          const timer=setTimeout(()=>{signal.removeEventListener('abort',onAbort);resolve();},3000);
          signal.addEventListener('abort',onAbort,{once:true});
        });
        if (token !== generation || signal.aborted) return;
      }
    }
    if (token !== generation) return;
    if (typeof session.socketPath !== 'string' || !session.socketPath.startsWith('/api/vm/desktop/ws?session=')) throw new Error('The desktop connection could not be opened.');
    const url = new URL(session.socketPath, location.origin); url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const client = new RFB(screen, url.href, { shared: true, credentials: session.credentials });
    session.credentials = null;
    rfb = client;
    trackGuestCursor(client);
    // The owner container's TigerVNC (Xvnc) drops Ctrl when noVNC sends key codes
    // (QEMU extended key events), though it handles plain keysyms like a native
    // viewer does. Other computers use Proxmox's QEMU VNC and keep key codes.
    // `&keys=keysym` / `&keys=code` force either mode for troubleshooting.
    const keyMode = query.get('keys') || (ownerMode ? 'keysym' : 'code');
    if (keyMode === 'keysym') {
      Object.defineProperty(client, '_qemuExtKeyEventSupported', { get: () => false, set: () => {} });
    }
    // `&debug=keys` shows the last key events this page sends, to tell a browser
    // or OS that swallows Ctrl apart from a server that drops it.
    if (query.get('debug') === 'keys') {
      // `raw` is what the browser delivered to the page, `sent` is what this page
      // forwarded to the VNC server.
      const log = [];
      const note = text => {
        log.push(text);
        if (log.length > 10) log.shift();
        $('desktop-notice').textContent = log.join(' | ');
      };
      if (!window.__keyDebugRaw) {
        window.__keyDebugRaw = true;
        for (const type of ['keydown', 'keyup']) {
          document.addEventListener(type, e => note(`raw ${type === 'keydown' ? 'down' : 'up'} ${e.code} ctrl=${e.ctrlKey ? 1 : 0}`), true);
        }
      }
      const original = client.sendKey.bind(client);
      client.sendKey = (keysym, code, down) => {
        note(`sent ${down === false ? 'up' : 'down'} ${code || '?'} 0x${(keysym || 0).toString(16)}`);
        return original(keysym, code, down);
      };
    }
    client.scaleViewport = true; client.resizeSession = true; client.clipViewport = false;
    client.showDotCursor = true; client.background = '#050607'; client.qualityLevel = 7; client.compressionLevel = 2;
    client.addEventListener('connect', () => {
      if (token !== generation) return;
      clearTimeout(connectTimer); connected = true; connecting = false; reconnectAttempts = 0;
      connectionState('Connected', 'connected'); cover.classList.add('is-hidden'); client.focus();
      if (!uptimeInterval) {
        uptimeInterval = setInterval(() => {
          if (connected && !leaseState.isExempt && leaseState.remainingSeconds > 0) {
            leaseState.remainingSeconds--;
            updateUptimeDisplay();
            checkUptimeAlerts();
          }
        }, 1000);
      }
    });
    client.addEventListener('disconnect', event => interrupted(token, event.detail.clean));
    client.addEventListener('securityfailure', () => {
      if (token !== generation) return;
      closeConnection(); connectionState('Disconnected', 'disconnected');
      showCover('Desktop connection expired', 'Reconnect to open a fresh secure connection.', true);
    });
    client.addEventListener('credentialsrequired', () => {
      if (token !== generation) return;
      closeConnection(); connectionState('Disconnected', 'disconnected');
      showCover('Desktop connection expired', 'Reconnect to open a fresh secure connection.', true);
    });
    connectTimer = setTimeout(() => interrupted(token), 20000);
  } catch (error) {
    if (token !== generation || error.name === 'AbortError') return;
    closeConnection(); connectionState('Disconnected', 'disconnected');
    showCover('Desktop unavailable', error.message, ![401, 403, 404].includes(error.status));
  }
}
async function power(action) {
  if (powerBusy) return;
  const labels = { restart: 'Restart this computer? Save your work first.', shutdown: 'Shut down this computer? Save your work first.', 'force-stop': 'Force stop this computer? Unsaved work may be lost and files could be damaged.' };
  if (!confirm(labels[action])) return;
  powerBusy = true; document.querySelectorAll('[data-power]').forEach(button => { button.disabled = true; });
  try {
    await request(ownerMode ? `${OWNER_API}/power` : `/api/vm/computers/${encodeURIComponent(id)}/power`, { method: 'POST', headers, body: JSON.stringify({ action }) });
    closeConnection();
    connectionState(action === 'restart' ? 'Restarting' : 'Shutting down');
    showCover(action === 'restart' ? 'Restarting your computer' : 'Shutting down your computer', action === 'restart' ? 'Reconnect in a moment when your desktop is ready.' : 'You can safely return to My Computer.', action === 'restart');
  } catch (error) { alert(error.message); }
  finally { powerBusy = false; document.querySelectorAll('[data-power]').forEach(button => { button.disabled = false; }); }
}

$('cad-button').addEventListener('click', () => rfb?.sendCtrlAltDel());
$('mobile-cad').addEventListener('click', () => { rfb?.sendCtrlAltDel(); $('power-menu').hidden = true; });
async function fullscreen() { try { if (document.fullscreenElement) await document.exitFullscreen(); else await frame.requestFullscreen(); } catch { showNotice('Fullscreen is not available in this browser.'); } }
function showNotice(message) { $('desktop-notice').textContent = message; setTimeout(() => { $('desktop-notice').textContent = ''; }, 5000); }
$('fullscreen-button').addEventListener('click', fullscreen); $('mobile-fullscreen').addEventListener('click', fullscreen);
const menu = $('power-menu'), more = $('more-button');
function hideMenu() { menu.hidden = true; more.setAttribute('aria-expanded', 'false'); }
more.addEventListener('click', () => { menu.hidden = !menu.hidden; more.setAttribute('aria-expanded', String(!menu.hidden)); });
menu.addEventListener('click', event => { const button = event.target.closest('[data-power]'); if (button) { hideMenu(); power(button.dataset.power); } });
document.addEventListener('click', event => { if (!menu.contains(event.target) && !more.contains(event.target)) hideMenu(); });
document.addEventListener('keydown', event => { if (event.key === 'Escape') hideMenu(); });

// Browser-drawn cursor: guest display adapters (virtio-gpu, VirGL, qxl —
// anything other than plain std) don't reliably hand a visible cursor
// shape to noVNC over VNC, and the browser's own pointer auto-hides
// after a moment in fullscreen — leaving nothing visible at all. Draw
// our own, positioned against display-frame (the element that actually
// goes fullscreen) so it works in both modes without separate
// fullscreen handling. Off switch lives in the 3-dot menu for anyone
// who'd rather see the guest's real cursor shape (e.g. resize handles)
// and accept the fullscreen tradeoff.
const localCursor = $('local-cursor'), cursorToggleBtn = $('cursor-toggle-button');
let localCursorEnabled = true;
try { localCursorEnabled = localStorage.getItem('desktop_local_cursor') !== 'off'; } catch {}
function applyCursorToggleUi() {
  if (!cursorToggleBtn) return;
  cursorToggleBtn.textContent = `Browser Cursor: ${localCursorEnabled ? 'On' : 'Off'}`;
  cursorToggleBtn.setAttribute('aria-checked', String(localCursorEnabled));
}
function hideLocalCursor() {
  localCursor?.classList.remove('is-active');
  screen.classList.remove('hide-native-cursor');
}
// The guest's own cursor shape (sent by the VNC server) is normally handed to
// the browser as a CSS cursor, which browsers hide in fullscreen. Intercept it
// and draw it in the page instead, so it follows the guest (text beam, resize
// arrows, ...) and stays visible. With no shape (empty or never sent) the
// default arrow from the stylesheet is used.
function trackGuestCursor(client) {
  const cursor = client._cursor;
  if (!cursor || typeof cursor.change !== 'function' || !localCursor) return;
  const original = cursor.change.bind(cursor);
  cursor.change = (rgba, hotX, hotY, w, h) => {
    original(rgba, hotX, hotY, w, h);
    if (!w || !h) { setGuestCursorImage(null); return; }
    try {
      const canvas = document.createElement('canvas');
      canvas.width = w; canvas.height = h;
      canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
      setGuestCursorImage({ url: canvas.toDataURL(), hotX, hotY, w, h });
    } catch { setGuestCursorImage(null); }
  };
}
function setGuestCursorImage(shape) {
  if (!localCursor) return;
  const style = localCursor.style;
  if (!shape) {
    style.width = style.height = style.margin = style.backgroundImage = style.backgroundSize = '';
    return;
  }
  style.width = `${shape.w}px`; style.height = `${shape.h}px`;
  style.margin = `${-shape.hotY}px 0 0 ${-shape.hotX}px`;
  style.backgroundImage = `url("${shape.url}")`; style.backgroundSize = '100% 100%';
}
// Fullscreen always uses the page-drawn cursor: that is where browsers hide the
// real one, so the Browser Cursor off switch only applies to the windowed view.
const useLocalCursor = () => localCursorEnabled || Boolean(document.fullscreenElement);
document.addEventListener('fullscreenchange', () => { if (!useLocalCursor()) hideLocalCursor(); });
applyCursorToggleUi();
// Capture phase, not bubble (the default): noVNC's RFB attaches its own
// mousemove handler directly to the canvas it creates inside #screen,
// and that handler unconditionally calls stopPropagation() on every
// mouse event (rfb.js's _handleMouse). A bubble-phase listener on
// #screen — an ancestor of that canvas — never sees the event once
// that fires; it's stopped before it bubbles back up. A capture-phase
// listener runs on the way *down* to the target, before the canvas's
// own bubble-phase handler executes at all, so it isn't affected by
// that stopPropagation() call.
screen.addEventListener('mousemove', event => {
  if (!useLocalCursor() || !connected || !localCursor) return;
  const rect = frame.getBoundingClientRect();
  localCursor.style.left = `${event.clientX - rect.left}px`;
  localCursor.style.top = `${event.clientY - rect.top}px`;
  localCursor.classList.add('is-active');
  screen.classList.add('hide-native-cursor');
}, true);
screen.addEventListener('mouseleave', hideLocalCursor);
cursorToggleBtn?.addEventListener('click', () => {
  localCursorEnabled = !localCursorEnabled;
  try { localStorage.setItem('desktop_local_cursor', localCursorEnabled ? 'on' : 'off'); } catch {}
  applyCursorToggleUi();
  if (!localCursorEnabled) hideLocalCursor();
  hideMenu();
});
const clipboard = $('clipboard-dialog'), clipboardText = $('clipboard-text');
function openClipboard() { hideMenu(); clipboard.returnValue = ''; clipboard.showModal(); }
$('clipboard-button').addEventListener('click', openClipboard); $('mobile-clipboard').addEventListener('click', openClipboard);
clipboard.addEventListener('close', () => { if (clipboard.returnValue === 'send' && connected) rfb?.clipboardPasteFrom(clipboardText.value); clipboardText.value = ''; if (connected) rfb?.focus(); });
const keyboard = $('mobile-keyboard');
const sentinel = '\u200b';
function resetKeyboard() { keyboard.value = sentinel; keyboard.setSelectionRange(1, 1); }
function sendText(value) { for (const ch of value) { const cp = ch.codePointAt(0); rfb?.sendKey(cp === 10 ? 0xff0d : cp > 255 ? 0x01000000 | cp : cp); } }
$('keyboard-button').addEventListener('click', () => { resetKeyboard(); keyboard.focus({ preventScroll: true }); });
keyboard.addEventListener('beforeinput', event => {
  if (!connected || event.isComposing) return;
  if (event.inputType === 'deleteContentBackward' || event.inputType === 'deleteContentForward') {
    event.preventDefault(); rfb.sendKey(event.inputType === 'deleteContentBackward' ? 0xff08 : 0xffff); resetKeyboard();
  } else if (event.inputType === 'insertLineBreak' || event.inputType === 'insertParagraph') { event.preventDefault(); rfb.sendKey(0xff0d); resetKeyboard(); }
});
keyboard.addEventListener('input', event => { if (!connected || event.isComposing) return; sendText(keyboard.value.replaceAll(sentinel, '')); resetKeyboard(); });
keyboard.addEventListener('compositionend', () => { if (connected) sendText(keyboard.value.replaceAll(sentinel, '')); resetKeyboard(); });
$('mobile-enter').addEventListener('click', () => rfb?.sendKey(0xff0d));
$('mobile-escape').addEventListener('click', () => rfb?.sendKey(0xff1b));
$('reconnect-button')?.addEventListener('click', () => { reconnectAttempts=0; connect(); });
$('retry-button').addEventListener('click', () => { reconnectAttempts = 0; connect(); });
window.addEventListener('pagehide', () => { disposed = true; closeConnection(); });
// Owner container: report real input so the server can shut it down after ten idle minutes.
if (ownerMode) {
  let lastPing = 0;
  const reportActivity = () => {
    const now = Date.now();
    if (!connected || now - lastPing < 30000) return;
    lastPing = now;
    fetch(`${OWNER_API}/activity`, { method: 'POST', credentials: 'same-origin', cache: 'no-store', headers: adminHeaders({ 'Content-Type': 'application/json' }), body: '{}' }).catch(() => {});
  };
  ['pointerdown', 'pointermove', 'keydown', 'wheel', 'touchstart'].forEach(type => document.addEventListener(type, reportActivity, { passive: true, capture: true }));
}
document.addEventListener('visibilitychange', () => {
  if (!document.hidden && resumeWhenVisible && !disposed && !connected && !connecting) { resumeWhenVisible = false; reconnectAttempts = 0; connect(); }
});
window.addEventListener('pageshow', event => { if (event.persisted) { disposed = false; reconnectAttempts = 0; connect(); } });
window.addEventListener('offline', () => { closeConnection(); connectionState('Offline', 'disconnected'); showCover('You are offline', 'Check your internet connection, then reconnect.', true); });
uptimeBtn?.addEventListener('click', openUptimeModal);
$('uptime-dismiss-btn')?.addEventListener('click', () => uptimeDialog?.close());
$('uptime-close-btn')?.addEventListener('click', () => uptimeDialog?.close());
$('uptime-extend-btn')?.addEventListener('click', extendSession);
$('mobile-extend')?.addEventListener('click', () => { hideMenu(); openUptimeModal(); });

connect();

