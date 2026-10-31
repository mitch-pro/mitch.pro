import RFB from '/novnc/core/rfb.js';

const $ = id => document.getElementById(id);
const BASE = '/api/admin/owner-container';
let rfb = null;

function headers() {
  const h = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
  const pass = sessionStorage.getItem('admin_passphrase');
  if (pass) h['X-Admin-Passphrase'] = pass;
  return h;
}

function say(text, isError) {
  const el = $('oc-message');
  el.textContent = text || '';
  el.classList.toggle('err', !!isError);
}

async function api(path, options) {
  const res = await fetch(BASE + path, { credentials: 'same-origin', cache: 'no-store', headers: headers(), ...options });
  const data = await res.json().catch(() => ({}));
  if (res.status === 403 && (data.error === 'invalid_passphrase' || data.error === 'passphrase_not_configured')) {
    throw new Error('Unlock your admin passphrase on the admin page first, then reload this page.');
  }
  if (!res.ok) throw new Error(data.error || 'Request failed (' + res.status + ').');
  return data;
}

function uptime(seconds) {
  const s = Math.floor(Number(seconds) || 0);
  if (!s) return '-';
  const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60);
  return (d ? d + 'd ' : '') + (h || d ? h + 'h ' : '') + m + 'm';
}

async function refresh() {
  try {
    const d = await api('', { method: 'GET' });
    if (!d.exists) {
      $('oc-card').hidden = true;
      say('Container 500 does not exist in Proxmox yet.', true);
      return;
    }
    say('');
    $('oc-card').hidden = false;
    $('oc-name').textContent = d.name || '-';
    $('oc-status').textContent = d.status || '-';
    $('oc-cpu').textContent = d.cpuCores ? d.cpuCores + ' cores' : '-';
    $('oc-mem').textContent = d.memoryMb ? d.memoryMb + ' MB' : '-';
    $('oc-disk').textContent = d.diskGb ? d.diskGb + ' GB' : '-';
    $('oc-uptime').textContent = d.status === 'running' ? uptime(d.uptime) : '-';
    $('oc-console').disabled = d.status !== 'running';
  } catch (err) {
    $('oc-card').hidden = true;
    say(err.message, true);
  }
}

async function power(action) {
  say('Sending ' + action + '...');
  try {
    await api('/power', { method: 'POST', body: JSON.stringify({ action }) });
    say('Sent. Status updates in a few seconds.');
    setTimeout(refresh, 3000);
  } catch (err) { say(err.message, true); }
}

async function openConsole() {
  say('Connecting...');
  try {
    const session = await api('/console', { method: 'POST', body: '{}' });
    if (typeof session.socketPath !== 'string' || !session.socketPath.startsWith('/api/vm/desktop/ws?session=')) {
      throw new Error('The console connection could not be opened.');
    }
    const url = new URL(session.socketPath, location.origin);
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const screen = $('oc-screen');
    if (rfb) { try { rfb.disconnect(); } catch (e) {} }
    screen.hidden = false;
    screen.innerHTML = '';
    rfb = new RFB(screen, url.href, { shared: true, credentials: session.credentials });
    rfb.scaleViewport = true;
    rfb.background = '#000000';
    rfb.addEventListener('disconnect', () => say('Console disconnected.'));
    rfb.addEventListener('connect', () => say(''));
  } catch (err) { say(err.message, true); }
}

document.querySelectorAll('[data-power]').forEach(b => b.addEventListener('click', () => power(b.dataset.power)));
$('oc-console').addEventListener('click', openConsole);
$('oc-refresh').addEventListener('click', refresh);
refresh();
