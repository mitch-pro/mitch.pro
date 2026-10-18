// Real local server and isolated fixture accounts; never production data.
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { createHash, createHmac } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { createServer } from 'node:net';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const data = await mkdtemp('/tmp/mitch-presence-browser-');
const socket = createServer();
await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
const port = socket.address().port; await new Promise(resolve => socket.close(resolve));
const base = 'http://127.0.0.1:' + port;
const server = spawn(root + '/rust/target/debug/mitch-server', [], {
  cwd: root, stdio: 'ignore', env: { ...process.env, DATA_DIR: data, MITCH_BASE: root,
    NODE_ENV: 'test', GOD_MODE: '0', PORT: String(port), VAPID_PUBLIC_KEY: '', RECAPTCHA_SITE_KEY: '',
    PVE_URL: 'https://127.0.0.1:1', PROXMOX_HOST: '127.0.0.1', PROXMOX_PORT: '1' }
});
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, message, timeout = 4000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await check()) return; await delay(80); }
  throw new Error(message);
}
let aliceBrowser, watcherBrowser;
try {
  await until(async () => { try { return (await fetch(base + '/enroll/')).ok; } catch { return false; } }, 'Server startup', 15000);
  const secret = await readFile(data + '/id_secret.key');
  const db = new DatabaseSync(data + '/mitchpro.db');
  const accounts = ['alice', 'bob', 'carol', 'admin'].map(name => {
    const email = 'presence-' + name + '@example.test';
    const raw = 'e' + createHash('sha256').update(email).digest('hex').slice(0, 24);
    const sid = raw + '.' + createHmac('sha256', secret).update(raw).digest('hex').slice(0, 16);
    db.prepare('INSERT INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)').run(sid, email, email, Date.now());
    db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run(sid, JSON.stringify({ email, used: true }), Date.now());
    return { email, sid };
  });
  const [alice, bob, carol, admin] = accounts;
  db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run('alias-token',JSON.stringify({email:'pre.sence-alice@example.test'}),Date.now());
  for (const [a, b] of [[alice, carol], [carol, alice]]) db.prepare('INSERT INTO friend_lists(email,data,updated_at) VALUES(?,?,?)').run(a.email, JSON.stringify([b.email]), Date.now());
  db.close();
  await writeFile(data + '/admins.json', JSON.stringify({ admins: [admin.email] }));
  [aliceBrowser, watcherBrowser] = await Promise.all([chromium.launch({ headless: true }), chromium.launch({ headless: true })]);
  async function contextFor(browser, user) {
    const context = await browser.newContext({ serviceWorkers: 'block', reducedMotion: 'reduce' });
    await context.addCookies([{ name: 'id', value: user.sid, url: base }]);
    await context.addInitScript(() => localStorage.setItem('_mitch_cookie_consent', 'accepted'));
    await context.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
    return context;
  }
  const actor = await contextFor(aliceBrowser, alice), observer = await contextFor(watcherBrowser, bob);
  await actor.addInitScript(() => {
    window.WebSocket = new Proxy(window.WebSocket, { construct(Native, args) {
      const socket = new Native(...args); (window.__testSockets ||= []).push(socket); return socket;
    } });
  });
  const carolApi = await contextFor(watcherBrowser, carol);
  const adminApi = await contextFor(watcherBrowser, admin);
  async function actorStatus(viewer = carolApi) {
    const response = await viewer.request.get(base + '/api/members');
    assert.equal(response.status(), 200);
    const body = await response.json();
    assert.equal(body.members.filter(member=>String(member.handle||'').includes('alice')).length,1,'Email aliases must not duplicate People');
    return body.members.find(member => String(member.email || '').includes('alice') || String(member.handle || '').includes('alice'));
  }
  const watcher = await observer.newPage(); const invalidations = [];
  watcher.on('websocket', ws => ws.on('framereceived', event => {
    try { const message = JSON.parse(String(event.payload)); if (message.type === 'presence_changed') invalidations.push(message); } catch {}
  }));
  await watcher.goto(base + '/');
  await watcher.waitForFunction(() => window.MitchPresence);
  const packets = [], pageErrors = [];
  actor.on('request', r => { if (r.url().endsWith('/api/presence/heartbeat')) packets.push({http:r.postData()}); });
  const page = await actor.newPage();
  page.on('pageerror', e => pageErrors.push(e.message));
  page.on('websocket', ws => ws.on('framesent', e => { try { packets.push({ws:JSON.parse(String(e.payload))}); } catch {} }));
  await page.goto(base + '/'); await page.bringToFront();
  await until(async () => (await actorStatus())?.page === '/', 'Initial homepage presence');
  console.log('PASS: initial authenticated page presence');
  const started = Date.now();
  await page.goto(base + '/members/');
  await until(async () => (await actorStatus())?.page === '/members/', 'Full-page navigation must update promptly').catch(async error => { console.log('DEBUG',JSON.stringify({url:page.url(),state:await page.evaluate(()=>({presence:!!window.MitchPresence,details:window.MitchPresence?.details()})),status:await actorStatus(),packets,pageErrors},null,2));throw error; });
  assert(Date.now() - started < 4000);
  await until(async () => invalidations.length > 0, 'Non-friends must receive presence invalidations');
  assert(invalidations.every(message => !('email' in message) && !('playing' in message)), 'Global invalidations must not disclose private identity or activity');
  const nonFriendView = await actorStatus(observer);
  assert.equal(nonFriendView.online, true);
  assert.equal(nonFriendView.page, '', 'Non-friends must not receive current page');
  assert.equal(nonFriendView.activity, '', 'Non-friends must not receive activity');
  assert.equal(nonFriendView.playing, '', 'Non-friends must not receive playing');
  const adminView = await actorStatus(adminApi);
  assert.equal(adminView.page, '/members/', 'Admins must receive current page');
  assert.equal(adminView.activity, 'Viewing people', 'Admins must receive activity');
  await until(async () => (await watcher.locator('#online-members-list').innerText()).includes('Online now'), 'Home People panel must show online status without leaking page to non-friends');
  assert(!(await watcher.locator('#online-members-list').innerText()).includes('Viewing people'), 'Non-friends must not see page activity on Home People panel');
  await page.evaluate(() => history.pushState({}, '', '/leaderboard/?token=private#private'));
  await until(async () => (await actorStatus())?.page === '/leaderboard/', 'SPA navigation');
  assert(!(JSON.stringify(await actorStatus()).includes('token=private')));
  console.log('PASS: navigation, SPA paths, friend/admin-only page visibility, and private query stripping');
  const second = await actor.newPage(); await second.goto(base + '/shop/'); await second.bringToFront();
  await until(async () => (await actorStatus())?.page === '/shop/', 'New foreground tab');
  await page.evaluate(() => window.MitchPresence.send());
  await actor.request.post(base + '/api/presence/heartbeat', { data: { playing: 'Chatting in Matrix' } });
  await delay(200);
  assert.equal((await actorStatus()).page, '/shop/', 'Background tab and legacy Matrix activity must not replace the foreground page');
  await page.bringToFront(); await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await until(async () => (await actorStatus())?.page === '/leaderboard/', 'Refocusing an older tab');
  await page.close(); await second.bringToFront();
  await until(async () => (await actorStatus())?.page === '/shop/', 'Closing one tab must fall back to the remaining tab');
  console.log('PASS: foreground selection, background heartbeat protection, legacy writer protection, and tab fallback');
  await second.goto(base + '/');
  await second.evaluate(() => showIframe('/games/adrian-clicker/', false));
  await until(async () => (await actorStatus())?.page?.startsWith('/games/adrian-clicker'), 'Embedded game presence');
  const frame = second.frames().find(frame => frame.url().includes('/games/adrian-clicker'));
  await frame.waitForFunction(() => window.MitchPresence);
  assert.equal(await frame.evaluate(() => window.MitchPresence.delegated), true);
  await frame.evaluate(() => { document.title = 'Chess'; history.pushState({}, '', '/games/chess/'); });
  await until(async () => (await actorStatus())?.page === '/games/chess/', 'Embedded SPA navigation must reach the parent reporter');
  await second.evaluate(() => goBack());
  await until(async () => (await actorStatus())?.page === '/', 'Closing the game must clear game activity');
  const friends = await carolApi.request.get(base + '/api/friends/list');
  const friend = (await friends.json()).friends.find(friend => friend.email === alice.email);
  assert(friend && friend.page === '/' && friend.activity === 'On the homepage');
  console.log('PASS: iframe delegation, iframe SPA navigation, closing a game, and consistent Friends API fields');
  await second.evaluate(() => {
    window.WebSocket = new Proxy(window.WebSocket, { construct() { throw new Error('Fixture: transport unavailable'); } });
    for (const socket of window.__testSockets || []) socket.close();
    window.MitchPresence.setTransport(() => false);
  });
  await delay(3700);
  assert.equal((await actorStatus()).page, '/', 'WebSocket cleanup must preserve a newer HTTP fallback heartbeat');
  await second.goto(base + '/vms/desktop/');
  await until(async () => (await actorStatus())?.activity === 'Using a VM desktop', 'VM desktop bypass pages must report presence');
  await second.goto(base + '/matrix/presence-fixture');
  await until(async () => (await actorStatus())?.page === '/matrix/presence-fixture', 'Matrix SPA fallback pages must report presence');
  assert.equal((await actorStatus()).activity, 'In Matrix chat');
  await watcher.goto(base + '/members/');
  await until(async () => (await watcher.locator('#membersGrid').innerText()).includes('Online now'), 'Full People page for non-friend must show online status only');
  assert(!(await watcher.locator('#membersGrid').innerText()).includes('In Matrix chat'), 'Full People page must not leak activity to non-friends');
  const friendWatcher = await carolApi.newPage();
  await friendWatcher.goto(base + '/friends/');
  await until(async () => (await friendWatcher.locator('#friendsGrid').innerText()).includes('In Matrix chat'), 'Full Friends page must show the same activity');
  await second.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pagehide', { persisted: true })));
  await until(async () => !(await actorStatus())?.online, 'Page cache suspension must end the document');
  await second.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })));
  await until(async () => (await actorStatus())?.page === new URL(second.url()).pathname, 'Page cache restoration must resume with a newer sequence').catch(async error => { console.log('RESTORE DEBUG', {url:second.url(),details:await second.evaluate(()=>window.MitchPresence?.details()),status:await actorStatus()}); throw error; });
  const content = await actor.request.post(base + '/api/content', { data: { hash: alice.sid, path: 'games/adrian-clicker/' } });
  const legacyHtml = (await content.json()).content;
  assert(legacyHtml.includes('data-presence-page="/games/adrian-clicker/index.html"'), 'Legacy content must report its real source page');
  assert(legacyHtml.indexOf('/presence.js') < legacyHtml.indexOf('</head>'));
  console.log('PASS: socket-to-HTTP fallback, VM desktops, Matrix SPA fallback, page cache restoration, and legacy embeds');
  await second.goto(base + '/');
  await until(async () => (await actorStatus())?.page === '/', 'Return from HTTP-only Matrix to a broadcast-connected page');
  await second.waitForFunction(() => window.__testSockets?.some(socket => socket.readyState === WebSocket.OPEN));
  await second.close();
  await until(async () => !(await actorStatus())?.online, 'Last-tab disconnect must go offline within 5 seconds', 5500).catch(async error => {console.log('CLOSE DEBUG',{status:await actorStatus(),pages:actor.pages().map(p=>p.url()),packets:packets.slice(-15)});throw error;});
  await until(async () => (await friendWatcher.locator('.friend-status').innerText()).includes('Offline'), 'Friends must clear activity after the last tab closes');
  const post = data => actor.request.post(base + '/api/presence/heartbeat', { data });
  const packet = { clientId: 'fallback-client', seq: 10, page: '/members/?secret=hidden', activity: 'Viewing people', visible: true, focused: true, interaction: 1 };
  assert.equal((await post(packet)).status(), 200);
  assert.equal((await post({ ...packet, seq: 9, page: '/shop/' })).status(), 200);
  assert.equal((await actorStatus()).page, '/members/');
  assert.equal((await post({ ...packet, seq: 11, ended: true })).status(), 200);
  assert.equal((await post(packet)).status(), 200);
  assert.equal((await actorStatus()).online, false, 'An old HTTP request must not revive a closed page');
  assert.equal((await post({ ...packet, seq: 12, page: '//external.example/' })).status(), 400);
  const anonymous = await watcherBrowser.newContext();
  assert([401,403].includes((await anonymous.request.post(base + '/api/presence/heartbeat', { data: packet })).status()));
  console.log('PASS: last-tab disconnect, HTTP fallback ordering, closed-page tombstones, invalid paths, and authentication');
  const revokeDb = new DatabaseSync(data+'/mitchpro.db');
  revokeDb.prepare('INSERT INTO revoked_ids(sid,data,updated_at) VALUES(?,?,?)').run(alice.sid,JSON.stringify({email:alice.email}),Date.now());revokeDb.close();
  assert([401,403].includes((await post({...packet,seq:20})).status()),'Revoked sessions must not publish presence');
  assert.deepEqual(pageErrors,[]);
  console.log('PASS: presence browser integration complete; no production accounts or messages used.');
} finally {
  await Promise.allSettled([aliceBrowser?.close(), watcherBrowser?.close()]);
  server.kill('SIGTERM');
  await new Promise(resolve => server.once('exit', resolve));
  await rm(data, { recursive: true, force: true });
}
