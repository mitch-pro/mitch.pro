import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const data = await mkdtemp('/tmp/mitch-game-discovery-');
const probe = createServer();
await new Promise(resolve => probe.listen(0, '127.0.0.1', resolve));
const port = probe.address().port;
await new Promise(resolve => probe.close(resolve));
const base = `http://127.0.0.1:${port}`;
const child = spawn(root + '/rust/target/debug/mitch-server', [], {
  cwd: root, stdio: ['ignore', 'ignore', 'pipe'],
  env: { ...process.env, DATA_DIR: data, MITCH_BASE: root, USERDATA_DIR: data + '/userdata',
    NODE_ENV: 'test', GOD_MODE: '0', PORT: String(port),
    PROXMOX_HOST: '127.0.0.1', PROXMOX_PORT: '1', PVE_URL: 'https://127.0.0.1:1',
    PVE_SSH_HOST: '127.0.0.1', PVE_SSH_PORT: '1', CONDUIT_HOST: '127.0.0.1', CONDUIT_PORT: '1',
    NTFY_TOPIC: '', NTFY_URL: '', VAPID_PRIVATE_KEY: '' }
});
let stderr = '', browser;
child.stderr.on('data', value => { stderr += value; });
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, message, timeout = 15000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await check()) return; await delay(100); }
  throw new Error(message);
}
const topSix = ['Run 3', 'Snow Rider 3D', 'Retro Bowl', 'Subway Surfers', 'Drift Boss', 'MotoX3M'];
try {
  await until(async () => { try { return (await fetch(base + '/api/access/status')).ok; } catch { return false; } }, 'Local fixture startup: ' + stderr);
  browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
  for (const path of ['/games', '/games/', '/games/index.html']) {
    const response = await fetch(base + path + '?q=run%203', { redirect: 'manual' });
    assert.equal(response.status, 302);
    assert.equal(response.headers.get('location'), '/game-portal/?q=run%203');
  }
  for (const width of [1440, 1024, 820, 390, 320]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, reducedMotion: 'reduce', serviceWorkers: 'block' });
    await context.addInitScript(() => localStorage.setItem('_mitch_cookie_consent', 'accepted'));
    await context.route('**/*', async route => {
      const url = new URL(route.request().url());
      if (url.origin !== base) return route.abort();
      if (url.pathname.startsWith('/proxy/')) {
        if (route.request().resourceType() === 'image') return route.fulfill({ contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect width="300" height="200" fill="#334348"/></svg>' });
        return route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Fixture game</title><p>Local game fixture</p>' });
      }
      if (url.pathname.startsWith('/api/game-portal/')) return route.fulfill({ json: { authenticated: true, coins: 120, dailyEarned: 0, dailyCap: 240, rewardPerMinute: 2, earned: 0 } });
      return route.continue();
    });
    const page = await context.newPage(), errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(base + '/game-portal/');
    await page.locator('#quick-grid .quick-card').nth(11).waitFor();
    assert.deepEqual(await page.locator('#quick-grid .card-info strong').allTextContents().then(titles => titles.slice(0, 6)), topSix);
    assert.deepEqual(await page.locator('#game-grid .card-info strong').allTextContents().then(titles => titles.slice(0, 6)), topSix);
    assert.equal(await page.locator('#game-grid .game-card').count(), 60);
    assert.equal(await page.locator('#quick-grid').getByRole('button', { name: 'Play Cookie Clicker', exact: true }).count(), 1, 'Popular games should prefer the embedded local copy');
    const popularTop = await page.locator('#quick-picks-section').boundingBox();
    assert(popularTop.y < 450, 'Popular games should be visible near the top');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'Page overflow at ' + width);
    assert(await page.getByRole('link', { name: 'Home', exact: true }).first().isVisible());
    await page.locator('#search').fill('moto x3m');
    assert(await page.locator('#quick-picks-section').isHidden());
    assert.equal(await page.locator('#game-grid .card-info strong').first().innerText(), 'MotoX3M');
    await page.locator('#search').fill('No matching fixture title zzz');
    assert(await page.locator('#empty-state').isVisible());
    await page.locator('#clear-filters').click();
    await page.locator('#filters').getByRole('button', { name: 'Puzzle', exact: true }).click();
    assert(await page.locator('#quick-picks-section').isHidden());
    assert((await page.locator('#game-grid .card-info small').allTextContents()).every(value => value.startsWith('Puzzle')));
    await page.locator('#filters').getByRole('button', { name: 'All', exact: true }).click();
    await page.locator('#sort-games').selectOption('alphabetical');
    const alphabetical = await page.locator('#game-grid .card-info strong').allTextContents();
    assert.deepEqual(alphabetical, [...alphabetical].sort((a, b) => a.localeCompare(b)));
    await page.locator('#sort-games').selectOption('popular');
    if (width === 1440) {
      await page.locator('#quick-grid').getByRole('button', { name: 'Save Retro Bowl', exact: true }).click();
      await page.locator('[data-view="favorites"]').click();
      assert.deepEqual(await page.locator('#game-grid .card-info strong').allTextContents(), ['Retro Bowl']);
      await page.locator('[data-view="all"]').click();
      await page.locator('#quick-grid').getByRole('button', { name: 'Play Run 3', exact: true }).click();
      assert(await page.locator('#player').isVisible());
      assert.equal(await page.locator('#player-title').innerText(), 'Run 3');
      assert.equal(await page.locator('.player-home').getAttribute('href'), '/');
      assert.equal(await page.locator('.player-home').getAttribute('target'), '_top');
      await page.locator('#player-back').click();
      await page.locator('#player').waitFor({ state: 'hidden' });
      assert.equal(await page.locator('#player-frame').getAttribute('src'), 'about:blank');
      await page.locator('#load-more').click();
      assert.equal(await page.locator('#game-grid .game-card').count(), 120);
      await page.evaluate(() => scrollTo(0, 0));
      await page.screenshot({ path: '/tmp/mitch-games-desktop.png' });
    }
    if (width <= 390) {
      await page.locator('#mobile-menu').click();
      await page.locator('[data-view="favorites"]').click();
      assert.equal(await page.locator('#mobile-menu').getAttribute('aria-expanded'), 'false');
      await page.locator('#mobile-menu').click();
      await page.locator('[data-view="all"]').click();
      await page.evaluate(() => scrollTo(0, 0));
      await page.screenshot({ path: `/tmp/mitch-games-${width}.png` });
    }
    assert.deepEqual(errors, []);
    await context.close();
    console.log('PASS: researched ordering, filters, search, sort and navigation at ' + width + 'px');
  }
  const context = await browser.newContext({ serviceWorkers: 'block' });
  await context.route('**/*', route => {
    const url = new URL(route.request().url());
    if (url.origin !== base) return route.abort();
    if (url.pathname.startsWith('/api/game-portal/')) return route.fulfill({ json: { authenticated: false } });
    if (url.pathname.startsWith('/proxy/')) return route.abort();
    return route.continue();
  });
  const page = await context.newPage();
  await page.goto(base + '/games/?q=retro%20bowl');
  await page.locator('#game-grid .game-card').first().waitFor();
  assert.equal(await page.locator('#search').inputValue(), 'retro bowl');
  assert.equal(await page.locator('#game-grid .card-info strong').first().innerText(), 'Retro Bowl');
  await until(async () => (await page.locator('#wallet-link').getAttribute('href')).startsWith('/enroll/?next='), 'Sign-in should return to games');
  await page.getByRole('link', { name: 'Home', exact: true }).first().click();
  await page.waitForURL(base + '/');
  await context.route(base + '/fixture-host', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><iframe title="Games library" src="/game-portal/" style="width:100%;height:900px"></iframe>' }));
  await page.goto(base + '/fixture-host');
  await page.frameLocator('iframe').getByRole('link', { name: 'Home', exact: true }).first().click();
  await page.waitForURL(base + '/');
  assert.equal(await page.locator('iframe[title="Games library"]').count(), 0, 'Home must navigate out of the embedded library');
  await context.close();
  console.log('PASS: legacy bookmarks, sign-in return URL and homepage navigation from an embedded library. No production requests or messages.');
} finally {
  await browser?.close();
  const stopped = once(child, 'exit'); child.kill('SIGTERM'); await stopped;
  await rm(data, { recursive: true, force: true });
}
