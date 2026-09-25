import { chromium } from '@playwright/test';
import { readFile } from 'node:fs/promises';

const browser = await chromium.launch({
  headless: true,
  executablePath: 'C:/Program Files/Google/Chrome/Application/chrome.exe',
});
const refreshMode = process.argv.includes('--refresh');
const page = await browser.newPage({ viewport: { width: Number(process.argv[2]) || 1440, height: 900 }, serviceWorkers: 'block' });
const requests = new Set();
const errors = [];
const sends = [];
page.on('pageerror', error => errors.push(error.stack || error.message));
page.on('response', response => {
  if (response.status() >= 400 && response.url().includes('/_matrix/')) requests.add(`${response.status()} ${new URL(response.url()).pathname}`);
});
await page.addInitScript(() => {
  window.__deletedMatrixDatabases = [];
  const deleteDatabase = indexedDB.deleteDatabase.bind(indexedDB);
  indexedDB.deleteDatabase = name => {
    window.__deletedMatrixDatabases.push(name);
    return deleteDatabase(name);
  };
  localStorage.setItem('mx_access_token', 'fixture-token');
  localStorage.setItem('mx_user_id', '@fixture:mitch.pro');
  localStorage.setItem('mx_device_id', 'FIXTUREDEVICE');
  localStorage.setItem('mx_hs_url', 'https://mitchdog.com');
  localStorage.setItem('mx_has_access_token', 'true');
  localStorage.setItem('mx_is_guest', 'false');
});

const event = (id, sender, body, age = 0) => ({
  type: 'm.room.message',
  event_id: id,
  sender,
  origin_server_ts: Date.now() - age,
  content: { msgtype: 'm.text', body },
});
const roomId = '!fixture:mitch.pro';
const sync = {
  next_batch: 'fixture-next',
  account_data: { events: [{ type: 'm.direct', content: {} }] },
  presence: { events: [] },
  to_device: { events: [] },
  device_lists: { changed: [], left: [] },
  device_one_time_keys_count: {},
  rooms: { join: {
    [roomId]: {
      state: { events: [
        { type: 'm.room.create', state_key: '', sender: '@fixture:mitch.pro', content: { creator: '@fixture:mitch.pro', room_version: '10' } },
        { type: 'm.room.name', state_key: '', sender: '@fixture:mitch.pro', content: { name: 'General' } },
        { type: 'm.room.topic', state_key: '', sender: '@fixture:mitch.pro', content: { topic: 'The mitch.pro community' } },
        { type: 'm.room.member', state_key: '@fixture:mitch.pro', sender: '@fixture:mitch.pro', content: { membership: 'join', displayname: 'You' } },
        { type: 'm.room.member', state_key: '@alex:mitch.pro', sender: '@alex:mitch.pro', content: { membership: 'join', displayname: 'Alex' } },
      ] },
      timeline: { limited: false, prev_batch: 'fixture-prev', events: [
        event('$fixture1', '@alex:mitch.pro', 'Anyone tried the new games?', 240000),
        event('$fixture2', '@fixture:mitch.pro', 'Yeah, I played with some friends after school.', 180000),
        event('$fixture3', '@alex:mitch.pro', 'Nice. Want to jump into a match later?', 90000),
      ] },
      ephemeral: { events: [] },
      account_data: { events: [] },
      unread_notifications: { notification_count: 0, highlight_count: 0 },
    },
  } },
};

await page.route('**/api/matrix/sso-status', route => route.fulfill({ json: refreshMode ? { authenticated: true, user_id: '@fixture:mitch.pro', username: 'fixture' } : { authenticated: false } }));
await page.route('**/api/matrix/sso-login', route => route.fulfill({ json: { access_token: 'refreshed-fixture-token', user_id: '@fixture:mitch.pro', device_id: 'FIXTUREDEVICE', base_url: 'https://mitchdog.com' } }));
await page.route('**/matrix/', async route => route.fulfill({ contentType: 'text/html', body: await readFile(new URL('../webserver/matrix/index.html', import.meta.url), 'utf8') }));
await page.route('**/matrix/matrix-design.css*', async route => route.fulfill({ contentType: 'text/css', body: await readFile(new URL('../webserver/matrix/matrix-design.css', import.meta.url), 'utf8') }));
if (refreshMode) await page.route('**/matrix/bundles/*/bundle.js', route => route.fulfill({ contentType: 'application/javascript', body: '' }));
await page.route('**/_matrix/**', route => {
  const url = new URL(route.request().url());
  const path = url.pathname;
  if (path.includes('/send/m.room.message/')) sends.push({ method: route.request().method(), body: route.request().postData() });
  let body = {};
  if (path.endsWith('/versions')) body = { versions: ['v1.1', 'v1.2', 'v1.3', 'v1.4', 'v1.5', 'v1.6', 'v1.7', 'v1.8', 'v1.9', 'v1.10', 'v1.11'], unstable_features: {} };
  else if (path.endsWith('/account/whoami')) {
    if (refreshMode && route.request().headers()['authorization'] === 'Bearer fixture-token') return route.fulfill({ status: 401, json: { errcode: 'M_UNKNOWN_TOKEN', error: 'Expired token' } });
    body = { user_id: '@fixture:mitch.pro', device_id: 'FIXTUREDEVICE' };
  }
  else if (path.endsWith('/sync')) body = sync;
  else if (path.endsWith('/joined_rooms')) body = { joined_rooms: [roomId] };
  else if (path.endsWith('/capabilities')) body = { capabilities: { 'm.change_password': { enabled: true }, 'm.room_versions': { default: '10', available: { '10': 'stable' } } } };
  else if (path.endsWith('/pushrules')) body = { global: { content: [], override: [], room: [], sender: [], underride: [] } };
  else if (path.includes('/filter')) body = { filter_id: 'fixture-filter' };
  else if (path.endsWith('/keys/upload')) body = { one_time_key_counts: {} };
  else if (path.endsWith('/keys/query')) body = { device_keys: {} };
  else if (path.endsWith('/keys/claim')) body = { one_time_keys: {} };
  else if (path.endsWith('/room_keys/version')) return route.fulfill({ status: 404, json: { errcode: 'M_NOT_FOUND', error: 'No key backup' } });
  else if (path.includes('/profile/')) body = { displayname: path.includes('fixture') ? 'You' : 'Alex' };
  else if (path.includes('/rooms/') && path.endsWith('/state')) body = sync.rooms.join[roomId].state.events;
  else if (path.includes('/rooms/') && path.endsWith('/joined_members')) body = { joined: { '@fixture:mitch.pro': { display_name: 'You' }, '@alex:mitch.pro': { display_name: 'Alex' } } };
  else if (path.includes('/rooms/') && path.endsWith('/members')) body = { chunk: sync.rooms.join[roomId].state.events.filter(item => item.type === 'm.room.member') };
  else if (path.includes('/rooms/') && path.endsWith('/messages')) body = { chunk: [], start: 'fixture-prev', end: 'fixture-end' };
  else if (path.includes('/send/m.room.message/')) body = { event_id: '$fixture-sent' };
  route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
});

await page.goto(`https://mitchdog.com/matrix/#/room/${roomId}`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(12000);
const ok = page.getByRole('button', { name: 'OK', exact: true });
if (await ok.count()) await ok.first().click();
const general = page.getByText('General', { exact: true });
if (await general.count() && await general.first().isVisible()) await general.first().click();
await page.waitForTimeout(2500);
const later = page.getByRole('button', { name: 'Later', exact: true });
if (await later.count()) await later.first().click();
const editor = page.locator('[contenteditable="true"]').first();
if (await editor.count()) {
  await editor.fill('Fixture send check');
  await editor.press('Enter');
  await page.waitForTimeout(700);
}
const layout = await page.evaluate(() => ['.mx_SpacePanel', '.mx_LeftPanel', '.mx_RoomView', '.mx_EventTile', '.mx_EventTile_line', '.mx_MessageComposer', '.mx_MessageComposer_input'].map(selector => {
  const node = document.querySelector(selector);
  const box = node?.getBoundingClientRect();
  return { selector, width: Math.round(box?.width || 0), left: Math.round(box?.left || 0), padding: node ? getComputedStyle(node).padding : '' };
}));
const session = await page.evaluate(() => ({ token: localStorage.getItem('mx_access_token'), deletedDatabases: window.__deletedMatrixDatabases }));
console.log(JSON.stringify({ url: page.url(), sends, session, layout, errors: errors.map(error => error.split('\n')[0]), failed: [...requests] }, null, 2));
if (process.argv.includes('--screenshot')) await page.screenshot({ path: `matrix-fixture-${process.argv[2] || '1440'}.png`, fullPage: true });
await browser.close();
if (errors.length) throw new Error('Matrix fixture had page errors');
if (refreshMode && (session.token !== 'refreshed-fixture-token' || session.deletedDatabases.length)) {
  throw new Error('SSO refresh did not preserve the Matrix device store');
}
if (!refreshMode && sends.length !== 1) throw new Error('Matrix composer did not send exactly one message');
