import { chromium } from '@playwright/test';
import { generateKeyPairSync } from 'node:crypto';
import { mkdir, writeFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';

const out = resolve('artifacts/ui-review');
await mkdir(out, { recursive: true });
const browser = await chromium.launch({ headless: true, channel: 'chromium' });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, serviceWorkers: 'block' });
const { privateKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
const jwk = privateKey.export({ format: 'jwk' });
const pub = Buffer.concat([Buffer.from([4]), Buffer.from(jwk.x, 'base64url'), Buffer.from(jwk.y, 'base64url')]).toString('hex');
const email = 'alex@example.test';
let coinBalance = 1250.25, coinStatus = 200, coinRequests = 0;
const members = [
  { email: 'jamie@example.test', nickname: 'Jamie Chen', displayName: 'Jamie Chen', handle: 'jamie', online: true, unread: 2, bio: 'Usually up for a game of chess.', lastSeen: Date.now(), pubKey: pub },
  { email: 'sam@example.test', nickname: 'Sam Rivera', displayName: 'Sam Rivera', handle: 'sam', online: true, lastSeen: Date.now(), pubKey: pub },
  { email: 'riley@example.test', nickname: 'Riley Morgan', displayName: 'Riley Morgan', handle: 'riley', online: false, lastSeen: Date.now() - 3600000, pubKey: pub },
  { email: 'avery@example.test', nickname: 'Avery Park', displayName: 'Avery Park', handle: 'avery', online: false, lastSeen: Date.now() - 7200000, pubKey: pub }
];
members.forEach(member => { member.lastMessage = { ts: Date.now() - 60000, text: 'Ready for a rematch?' }; });
const destinations = [['matrix', 'Chat'], ['friends', 'Friends'], ['members', 'Members'], ['leaderboard', 'Leaderboard'], ['games', 'Games'], ['canvas', 'Canvas'], ['shop', 'Shop'], ['marketplace', 'Marketplace'], ['profile', 'Profile'], ['preferences', 'Preferences'], ['inventory', 'Inventory'], ['bell', 'Bell Schedule'], ['vms', 'Virtual Machines'], ['feedback', 'Feedback'], ['invite', 'Invite Friends']];
await context.addInitScript(({ jwk, email }) => {
  if (!['127.0.0.1', 'localhost'].includes(location.hostname)) return;
  localStorage.setItem('_e2e_private_jwk_v3:' + encodeURIComponent(email), JSON.stringify(jwk));
  localStorage.setItem('_prefHomepage', JSON.stringify({ greetingName: 'Alex' }));
  localStorage.setItem('_mitch_cookie_consent', 'accepted');
}, { jwk, email });
await context.route('**/*', async route => {
  const url = new URL(route.request().url());
  if (!['127.0.0.1', 'localhost'].includes(url.hostname)) return route.abort();
  if (!url.pathname.startsWith('/api/')) return route.continue();
  if (url.pathname === '/api/me/coins') {
    coinRequests++;
    return route.fulfill({ status: coinStatus, contentType: 'application/json', body: JSON.stringify(coinStatus === 200 ? { coins: coinBalance, achievements: [], stats: {} } : { error: 'unavailable' }) });
  }
  let data = { success: true, ok: true, items: [], notifications: [], requests: [], members: [], messages: [], groups: [], listings: [], friends: [], games: [], entries: [], events: [], profile: {} };
  if (url.pathname === '/api/me') data = { email, nickname: 'Alex', pubKeyHex: pub, isAdmin: false, isPremium: false, coins: 1250, balance: 1250 };
  if (url.pathname === '/api/members') data = { members };
  if (url.pathname === '/api/pass') data = { success: true, content: destinations.map(([href, name]) => `url /${href}/ ${name}`).join('\n') };
  if (url.pathname === '/api/profile') data = { email, nickname: 'Alex', name: 'Alex', bio: 'A little bit of everything.', coins: 1250, balance: 1250, handle: 'alex', profile: { nickname: 'Alex', bio: 'A little bit of everything.' } };
  if (url.pathname === '/api/friends/list') data = { friends: members.slice(0, 2) };
  if (url.pathname === '/api/vm/computers') data = { serviceAvailable: true, computers: [{ id: 'vm-preview', name: 'My Computer', hostname: 'alex-desktop', operatingSystem: 'Ubuntu Desktop 24.04 LTS', status: 'running', cpuCores: 4, cpuUsage: .18, memoryUsed: 2.4 * 1073741824, memoryTotal: 4 * 1073741824, diskUsed: 11 * 1073741824, diskTotal: 40 * 1073741824, ipAddress: '10.0.0.228', uptime: 17420, desktopAvailable: true }] };
  if (url.pathname === '/api/dm/groups') data = { groups: [{ id: 'study', name: 'After school', members: [email, members[0].email, members[1].email] }] };
  if (url.pathname === '/api/dm/inbox') data = { messages: url.searchParams.has('with') ? [
    { id: 'sample1', from: members[0].email, to: email, text: 'Are you around after school?', ts: Date.now() - 600000 },
    { id: 'sample2', from: email, to: members[0].email, text: 'Yeah! Thinking of checking out the new games.', ts: Date.now() - 480000 },
    { id: 'sample3', from: members[0].email, to: email, text: 'Chess rematch? I have been practicing.', ts: Date.now() - 120000 }
  ] : [] };
  if (url.pathname === '/api/vm/status') data = { hasVm: false };
  return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(data) });
});
const page = await context.newPage();
const errors = [];
page.on('pageerror', error => errors.push(error.message));
const routes = process.argv.includes('--all')
  ? ['/', '/index-sales.html', '/maintenance.html', ...(await readdir('webserver', { withFileTypes: true })).filter(x => x.isDirectory()).map(x => '/' + x.name + '/')]
  : ['/', '/matrix/', '/game-portal/', '/preferences/', '/members/', '/shop/', '/marketplace/', '/inventory/', '/leaderboard/', '/invite/', '/casino/', '/profile/', '/enroll/', '/bell/', '/feedback/', '/faq/', '/admin/', '/vms/', '/vms/desktop/', '/index-sales.html'];
const results = [];
for (const route of routes) {
  errors.length = 0;
  try {
    const response = await page.goto('http://127.0.0.1:4317' + route, { waitUntil: 'domcontentloaded', timeout: 15000 });
    if (response.status() === 404) continue;
    await page.locator('body').waitFor();
    await page.waitForTimeout(350);
    if (!route.startsWith('/rjuhsd') && await page.locator('.mitch-wallet').count()) {
      await page.waitForFunction(() => document.querySelector('.mitch-wallet')?.dataset.state === 'ready');
      if (!(await page.locator('.mitch-wallet img').first().evaluate(el => el.complete && el.naturalWidth > 0))) throw new Error('Coin image did not load');
    }
    const state = await page.evaluate(() => ({ title: document.title, width: innerWidth, scrollWidth: document.documentElement.scrollWidth, designed: document.body.classList.contains('mitch-design'), text: document.body.innerText.slice(0, 130) }));
    const slug = route.replaceAll('/', '_').replaceAll('.html', '') || 'home';
    await page.screenshot({ path: `${out}/${slug}-desktop.png`, fullPage: false });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: `${out}/${slug}-mobile.png`, fullPage: false });
    const mobile = await page.evaluate(() => ({ width: innerWidth, scrollWidth: document.documentElement.scrollWidth }));
    for (const width of [320, 768, 1024]) {
      await page.setViewportSize({ width, height: 844 });
      const walletHeaders = await page.locator('.has-mitch-wallet').evaluateAll(headers => headers.map(header => {
        const rects = [...header.children].map(child => child.getBoundingClientRect()).filter(r => r.width && r.height);
        return { id: header.id || header.className, fits: rects.every(r => r.left >= -1 && r.right <= innerWidth + 1), overlaps: rects.some((r, i) => rects.slice(i + 1).some(b => Math.min(r.right, b.right) > Math.max(r.left, b.left) + 1 && Math.min(r.bottom, b.bottom) > Math.max(r.top, b.top) + 1)) };
      }));
      if (walletHeaders.some(header => !header.fits || header.overlaps)) throw new Error('Wallet header collision at ' + width + ': ' + JSON.stringify(walletHeaders));
    }
    results.push({ route, ...state, mobile, errors: [...errors] });
    await page.setViewportSize({ width: 1440, height: 1000 });
  } catch (error) { results.push({ route, error: error.message }); }
}
await writeFile(`${out}/review.json`, JSON.stringify(results, null, 2));
await page.goto('http://127.0.0.1:4317/');
await page.waitForFunction(() => document.querySelector('.mitch-wallet')?.dataset.state === 'ready');
if (!(await page.locator('.mitch-wallet').getAttribute('title')).includes('1,250.25')) throw new Error('Wallet does not show the spendable balance');
coinBalance = 950.25;
await page.evaluate(async () => {
  const response = await fetch('/api/shop/buy', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  if (!(await response.json()).success) throw new Error('Wallet monitoring consumed the purchase response');
});
await page.waitForFunction(() => document.querySelector('.mitch-wallet-value').textContent === '950.25');
await page.reload();
await page.waitForFunction(() => document.querySelector('.mitch-wallet-value')?.textContent === '950.25');
await page.addScriptTag({ url: 'http://127.0.0.1:4317/mitch-coins.js?v=duplicate' });
if (await page.locator('.mitch-wallet').count() !== 1) throw new Error('Duplicate wallets after repeated initialization');
coinStatus = 401;
await page.evaluate(() => window.MitchCoins.refresh());
if (await page.locator('.mitch-wallet').getAttribute('href') !== '/enroll/') throw new Error('Signed-out wallet retained account data');
coinStatus = 500;
await page.evaluate(() => window.MitchCoins.refresh());
if (await page.locator('.mitch-wallet-value').textContent() !== '—') throw new Error('Failed balance request displayed an invented balance');
coinStatus = 200; coinBalance = 1234567.89;
await page.evaluate(() => window.MitchCoins.refresh());
for (const width of [320, 390, 480, 768]) {
  await page.setViewportSize({ width, height: 844 });
  const bounds = await page.locator('.home-masthead').evaluate(el => {
    const rects = [...el.children].map(child => child.getBoundingClientRect()).filter(r => r.width && r.height);
    return { fits: rects.every(r => r.left >= 0 && r.right <= innerWidth), overlaps: rects.some((r, i) => rects.slice(i + 1).some(b => Math.min(r.right, b.right) > Math.max(r.left, b.left) + 1 && Math.min(r.bottom, b.bottom) > Math.max(r.top, b.top) + 1)) };
  });
  if (!bounds.fits || bounds.overlaps) throw new Error('Wallet crowded mobile header: ' + width + ' ' + JSON.stringify(bounds));
}
coinBalance = 1250.25;
await page.evaluate(() => window.MitchCoins.refresh());
for (const width of [1024, 1440, 1920]) {
  await page.setViewportSize({ width, height: 1000 });
  const bar = await page.locator('.home-masthead').evaluate(el => {
    const box = el.getBoundingClientRect();
    const content = document.querySelector('#mainpage').getBoundingClientRect();
    const rail = document.querySelector('#member-side-rail').getBoundingClientRect();
    const children = [...el.children].map(child => child.getBoundingClientRect()).filter(r => r.width && r.height);
    return { left: box.left, width: box.width, bottom: box.bottom, contentTop: content.top, railTop: rail.top, childrenFit: children.every(r => r.left >= box.left && r.right <= box.right), overlaps: children.some((r, i) => children.slice(i + 1).some(b => Math.min(r.right, b.right) > Math.max(r.left, b.left) + 1 && Math.min(r.bottom, b.bottom) > Math.max(r.top, b.top) + 1)) };
  });
  if (Math.abs(bar.left) > 1 || Math.abs(bar.width - width) > 1 || bar.contentTop < bar.bottom - 1 || bar.railTop < bar.bottom - 1 || !bar.childrenFit || bar.overlaps) throw new Error('Desktop topbar regression: ' + JSON.stringify(bar));
}
await page.setViewportSize({ width: 1440, height: 1000 });
await page.screenshot({ path: `${out}/homepage-topbar-desktop.png` });
if (await page.locator('#site-topbar').count() !== 1) throw new Error('Duplicate homepage topbars');
await page.locator('#sw-notif-btn').click();
const notificationBounds = await page.locator('#sw-notif-panel').evaluate(el => { const r = el.getBoundingClientRect(); return { left: r.left, right: r.right, width: innerWidth }; });
if (notificationBounds.left < 0 || notificationBounds.right > notificationBounds.width) throw new Error('Notification panel escaped viewport');
await page.locator('#sw-notif-close').click();
await page.keyboard.press('/');
if (!(await page.locator('#home-search').evaluate(el => el === document.activeElement))) throw new Error('Search shortcut did not focus search');
await page.locator('#home-search').fill('matrix');
await page.keyboard.press('Escape');
if (await page.locator('#home-search').inputValue()) throw new Error('Escape did not reset search');
await context.addCookies([{ name: 'theme', value: 'light', url: 'http://127.0.0.1:4317' }]);
for (const route of ['/', '/matrix/', '/preferences/', '/index-sales.html']) {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('http://127.0.0.1:4317' + route);
  if (!(await page.locator('html').evaluate(el => el.classList.contains('theme-light')))) throw new Error('Light theme not applied');
  await page.screenshot({ path: `${out}/${route.replaceAll('/', '_').replace('.html', '')}-light.png` });
}
await writeFile(`${out}/review.json`, JSON.stringify(results, null, 2));
if (!process.argv.includes('--all')) {
  const failures = results.filter(result => result.error || result.errors?.length || !result.designed || result.scrollWidth > result.width || result.mobile?.scrollWidth > result.mobile?.width);
  if (failures.length) throw new Error('UI audit failed: ' + failures.map(result => result.route).join(', '));
}
console.log(JSON.stringify(results.map(({ route, designed, scrollWidth, width, mobile, errors, error }) => ({ route, designed, overflow: scrollWidth > width, mobileOverflow: mobile?.scrollWidth > mobile?.width, errors, error })), null, 2));
await browser.close();
