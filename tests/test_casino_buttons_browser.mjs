import assert from 'node:assert/strict';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');

const root = join(import.meta.dirname, '..');
const browser = await chromium.launch({
  executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH || (process.platform === 'win32'
    ? 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'
    : undefined),
  headless: true,
});
try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    window.__casinoPosts = [];
    window.getCaptchaToken = async () => 'test-token';
    window.fetch = async (url, options = {}) => {
      const path = String(url);
      if (options.method === 'POST') window.__casinoPosts.push({ path, body: JSON.parse(options.body) });
      const data = path.includes('/api/me/coins') ? { authenticated: true, coins: 1250 }
        : path.includes('/api/casino/slots') ? { rank: 'Test', win: 0, mult: 0, net: -1250, results: ['7', '7', '7'], newBalance: 0 }
        : path.includes('/api/casino/blackjack/state') ? { active: false }
        : path.includes('/api/casino/history') ? { history: [] }
        : path.includes('/api/casino/global-feed') ? { feed: [] }
        : {};
      return new Response(JSON.stringify(data), { status: 200, headers: { 'Content-Type': 'application/json' } });
    };
  });
  await page.goto(pathToFileURL(join(root, 'webserver/casino/index.html')).href);
  await page.waitForFunction(() => document.querySelector('#balance').textContent.includes('1,250'));
  const cards = await page.locator('.games > .card').evaluateAll(els => els.map(el => el.id));
  assert.equal(cards.length, 16);
  for (const id of cards) {
    await page.locator(`#${id} .controls > button`).click();
    assert.equal(await page.locator('#fullscreen-overlay').evaluate(el => el.classList.contains('active')), true, id);
    assert.equal(await page.locator('.fs-controls-box .btn.primary').last().isVisible(), true, id);
    if (id === 'card-slots') {
      await page.locator('#bet-max').click();
      assert.equal(await page.locator('#bet').inputValue(), '500');
      await page.locator('#slots').click();
      await page.waitForFunction(() => window.__casinoPosts.length > 0);
      const posted = await page.evaluate(() => window.__casinoPosts[0]);
      assert.equal(posted.path, '/api/casino/slots');
      assert.equal(posted.body.amount, 500);
      assert.equal(posted.body.recaptcha_token, 'test-token');
    }
    await page.locator('#fs-close-btn').click();
    assert.equal(await page.locator('#fullscreen-overlay').evaluate(el => el.classList.contains('active')), false, id);
  }
  assert.deepEqual(errors, []);
  console.log('Casino buttons: all 16 rooms, max wager, spin request, and return controls passed.');
} finally {
  await browser.close();
}
