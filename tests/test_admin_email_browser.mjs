import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, symlink, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { chromium } from '@playwright/test';

const repo = resolve(import.meta.dirname, '..');
const fixture = await mkdtemp(join(tmpdir(), 'mitch-admin-email-'));
const messages = [];
let rejectSend = false;
const mail = createServer(async (req, res) => {
  res.setHeader('Content-Type', 'application/json');
  if (req.url === '/status') {
    res.end(JSON.stringify({ ok: true, senders: { noreply: true, gmail: false, support: false } }));
  } else if (req.url === '/send' && req.method === 'POST') {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    messages.push(JSON.parse(Buffer.concat(chunks)));
    res.statusCode = rejectSend ? 500 : 200;
    res.end(JSON.stringify({ ok: !rejectSend }));
  } else { res.statusCode = 404; res.end('{}'); }
});
mail.listen(0, '127.0.0.1');
await once(mail, 'listening');
const portProbe = createServer();
portProbe.listen(0, '127.0.0.1');
await once(portProbe, 'listening');
const port = portProbe.address().port;
await new Promise(resolve => portProbe.close(resolve));
const base = `http://127.0.0.1:${port}`;
await mkdir(join(fixture, 'data'));
await symlink(join(repo, 'webserver'), join(fixture, 'webserver'));
await writeFile(join(fixture, 'data/site.json'), JSON.stringify({ primary: base, alternate: base }));
await writeFile(join(fixture, 'data/profiles.json'), JSON.stringify({
  'alice@example.test': { username: 'alice', displayName: 'Alice' },
  'bob@example.test': { username: 'bob', displayName: 'Bob' },
  'unsub@example.test': { username: 'unsub', displayName: 'Unsubscribed member' },
}));
await writeFile(join(fixture, 'data/newsletter_unsub.json'), JSON.stringify(['unsub@example.test']));
const server = spawn(join(repo, 'rust/target/debug/mitch-server'), [], {
  cwd: fixture,
  env: { ...process.env, NODE_ENV: 'test', DEV_TEST_ACCESS: '1', MITCH_BASE: fixture,
    DATA_DIR: join(fixture, 'data'), MITCH_DB_PATH: join(fixture, 'data/test.db'),
    PORT: String(port), MAIL_RS_URL: `http://127.0.0.1:${mail.address().port}`, RUST_LOG: 'error' },
  stdio: ['ignore', 'ignore', 'pipe'],
});
let serverErrors = '';
server.stderr.on('data', chunk => { serverErrors += chunk; });
let browser;
try {
  for (let attempt = 0; ; attempt++) {
    try { if ((await fetch(base + '/api/dev/test-access')).ok) break; } catch {}
    assert(attempt < 80, 'Local server failed to start: ' + serverErrors);
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  await context.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
  // This suite tests email; the admin shell also requests the unrelated Matrix service.
  await context.route(base + '/api/matrix/**', route => route.fulfill({
    contentType: 'application/json', body: JSON.stringify({ ok: true, staff: [], rooms: [] }),
  }));
  const headers = { Origin: base, 'X-Mitch-Requested-With': '1' };
  assert((await context.request.post(base + '/api/dev/test-access', { headers })).ok());
  const passphrase = randomUUID();
  assert((await context.request.post(base + '/api/admin/passphrase-status', { headers, data: { passphrase } })).ok());
  await context.addInitScript(pass => {
    if (window.top === window) sessionStorage.setItem('admin_passphrase', pass);
  }, passphrase);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(base + '/admin/#email-tools');
  await page.getByText('Connected to mitch-mail-rs.', { exact: true }).waitFor();
  assert.equal(messages.length, 0, 'Opening the tools must not send email.');
  assert.equal(await page.locator('#email-preview').getAttribute('sandbox'), '');
  await page.frameLocator('#email-preview').getByText('Test email only.', { exact: false }).waitFor();
  await page.getByRole('link', { name: 'Send email', exact: true }).click();
  await page.waitForURL(base + '/admin/email/');
  await page.getByText('Preview only. Nothing has been sent.', { exact: true }).waitFor();
  assert.equal(await page.locator('#newsletter-preview').getAttribute('sandbox'), '');
  assert.equal(messages.length, 0);
  assert.equal(await page.locator('#edition').inputValue(), 'October 16–18, 2026');
  assert.equal(await page.locator('#signature').inputValue(), 'Mitch & Tyler');
  assert.equal(await page.locator('.content-section').count(), 8);
  assert.equal(await page.getByRole('tab', { name: 'Message', exact: true }).getAttribute('aria-selected'), 'true');
  const messageTab = page.getByRole('tab', { name: 'Message', exact: true });
  const appearanceTab = page.getByRole('tab', { name: 'Appearance', exact: true });
  const recipientsTab = page.getByRole('tab', { name: 'Recipients', exact: true });
  await messageTab.focus();
  await page.keyboard.press('ArrowRight');
  assert.equal(await appearanceTab.getAttribute('aria-selected'), 'true');
  await page.keyboard.press('End');
  assert.equal(await recipientsTab.getAttribute('aria-selected'), 'true');
  await page.keyboard.press('Home');
  assert.equal(await messageTab.getAttribute('aria-selected'), 'true');
  const original = await page.locator('#newsletter-preview').getAttribute('srcdoc');
  if (process.env.NEWSLETTER_ARTIFACT_DIR) {
    const out = process.env.NEWSLETTER_ARTIFACT_DIR;
    await mkdir(out, { recursive: true });
    await writeFile(join(out, 'example.html'), original);
    const sample = await context.newPage();
    await sample.setViewportSize({ width: 760, height: 1000 });
    await sample.setContent(original);
    await sample.screenshot({ path: join(out, 'example.png'), fullPage: true });
    await sample.close();
    await page.bringToFront();
    await page.locator('h1').click();
    await page.screenshot({ path: join(out, 'editor.png') });
  }
  await page.locator('#review-email').click();
  await page.getByText('Choose at least one recipient who has not unsubscribed.', { exact: true }).waitFor();
  assert.equal(await recipientsTab.getAttribute('aria-selected'), 'true');
  await messageTab.click();
  await page.locator('#subject').fill('October updates — local test');
  await page.locator('#title').fill('A newsletter you can customize.');
  await page.locator('#intro').fill('Hello everyone,\n\nThis is an isolated preview test.');
  await recipientsTab.click();
  const alice = page.locator('.recipient-option').filter({ hasText: 'alice@example.test' }).locator('input');
  await alice.check();
  assert(await page.locator('.recipient-option').filter({ hasText: 'unsub@example.test' }).locator('input').isDisabled());
  await page.locator('#extra-recipients').fill('person@example.test, PERSON@example.test');
  await page.locator('#save-draft').click();
  await page.waitForFunction(() => document.getElementById('draft-status').textContent.startsWith('Saved '));
  await page.reload();
  await page.getByText('Preview only. Nothing has been sent.', { exact: true }).waitFor();
  assert.equal(await page.locator('#subject').inputValue(), 'October updates — local test');
  assert(await page.locator('.recipient-option').filter({ hasText: 'alice@example.test' }).locator('input').isChecked());
  assert.equal(await page.locator('#recipient-count').innerText(), '2 recipients');
  const heading = page.locator('[data-section-title]').first();
  const firstHeading = await heading.inputValue();
  const secondHeading = await page.locator('[data-section-title]').nth(1).inputValue();
  await page.locator('.content-section').nth(1).getByRole('button', { name: 'Move up', exact: true }).click();
  assert.equal(await heading.inputValue(), secondHeading);
  await page.locator('.content-section').first().getByRole('button', { name: 'Move down', exact: true }).click();
  assert.equal(await heading.inputValue(), firstHeading);
  await page.getByRole('button', { name: 'Phone', exact: true }).click();
  assert(await page.locator('#preview-canvas').evaluate(el => el.classList.contains('phone')));
  await appearanceTab.click();
  await page.selectOption('#theme', 'midnight');
  await page.waitForFunction(() => document.getElementById('newsletter-preview').srcdoc.includes('#111b2e'));
  await page.selectOption('#theme', 'paper');
  await page.waitForFunction(() => document.getElementById('newsletter-preview').srcdoc.includes('bgcolor="#ffffff"'));
  await page.locator('#heroUrl').fill('not-a-url');
  await messageTab.click();
  await page.locator('#review-email').click();
  assert.equal(await appearanceTab.getAttribute('aria-selected'), 'true', 'Invalid hidden fields must open their own tab.');
  await page.locator('#heroUrl').fill('');
  await page.locator('#review-email').click();
  await page.locator('#review-dialog[open]').waitFor();
  assert.equal(await page.locator('#review-count').innerText(), '2 recipients');
  assert.deepEqual(await page.locator('#review-recipients li').allTextContents(), ['alice@example.test', 'person@example.test']);
  assert.equal(messages.length, 0, 'Saving, previewing and reviewing must never send email.');
  await page.getByRole('button', { name: 'Keep editing', exact: true }).click();
  await recipientsTab.click();
  await page.locator('[name="audience"][value="everyone"]').check();
  await page.locator('#review-email').click();
  await page.locator('#review-dialog[open]').waitFor();
  const everyone = await page.locator('#review-recipients li').allTextContents();
  assert(everyone.includes('alice@example.test') && everyone.includes('bob@example.test'));
  assert(!everyone.includes('unsub@example.test'));
  assert(!everyone.includes('person@example.test'), 'Everyone must use registered accounts, not custom addresses from another mode.');
  assert.equal(messages.length, 0);
  await page.getByRole('button', { name: 'Keep editing', exact: true }).click();
  await page.locator('[name="audience"][value="selected"]').check();
  await page.locator('#review-email').click();
  await page.locator('#review-dialog[open]').waitFor();
  await page.locator('#send-newsletter').click();
  await page.waitForFunction(() => document.getElementById('delivery-status').textContent.startsWith('Finished.'));
  assert.equal(messages.length, 2);
  assert.deepEqual(messages.map(m => m.to).sort(), ['alice@example.test', 'person@example.test']);
  assert(messages.every(m => !m.cc && !m.bcc && m.raw && m.body.includes('/unsubscribe/')));
  const jobId = await page.evaluate(() => sessionStorage.getItem('newsletter_last_job'));
  const replay = await context.request.post(base + '/api/admin/newsletters/send', { headers: { ...headers, 'X-Admin-Passphrase': passphrase }, data: { id: jobId, confirm: true } });
  assert(replay.ok());
  assert.equal(messages.length, 2, 'Submitting the same reviewed newsletter again must not resend.');
  rejectSend = true;
  await page.locator('#review-email').click();
  await page.locator('#review-dialog[open]').waitFor();
  await page.locator('#send-newsletter').click();
  await page.waitForFunction(() => document.getElementById('delivery-status').textContent.includes('2 failed'));
  const messagesBeforeReset = messages.length;
  await messageTab.click();
  await page.locator('#load-example').click();
  assert.equal(await page.locator('#title').inputValue(), 'This weekend on mitch.pro');
  assert.equal(await page.locator('#signature').inputValue(), 'Mitch & Tyler');
  assert.equal(await page.locator('#recipient-count').textContent(), '2 recipients', 'Loading the new default preserves recipients.');
  await page.getByRole('button', { name: 'Desktop', exact: true }).click();
  await page.waitForFunction(() => document.getElementById('newsletter-preview').srcdoc.includes('This weekend on mitch.pro'));
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `Overflow at ${width}px`);
    assert(await page.frameLocator('#newsletter-preview').locator('html').evaluate(el => el.scrollWidth <= innerWidth + 1), `Email overflow at ${width}px`);
  }
  if (process.env.EMAIL_SCREENSHOT) await page.screenshot({ path: process.env.EMAIL_SCREENSHOT, fullPage: true });
  const previewTop = await page.frameLocator('#newsletter-preview').locator('h1').evaluate(el => el.getBoundingClientRect().top);
  assert(previewTop < 220, `Newsletter heading should be visible near the top of the preview, got ${previewTop}px`);
  assert.deepEqual(errors, []);
  assert.equal(messages.length, messagesBeforeReset, 'Loading a default and changing preview size must not send.');
  console.log('PASS: admin authentication, composer navigation, example/live previews, draft save/reload, selected/everyone audiences, unsubscribe exclusions, review without sending, section ordering, local fake delivery, duplicate prevention, SMTP failures and responsive layout. No real emails were sent.');
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  if (server.exitCode === null) await once(server, 'exit');
  await new Promise(resolve => mail.close(resolve));
  await rm(fixture, { recursive: true, force: true });
}
