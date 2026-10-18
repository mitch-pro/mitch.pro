import assert from 'node:assert/strict';
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
  const headers = { Origin: base, 'X-Mitch-Requested-With': '1' };
  assert((await context.request.post(base + '/api/dev/test-access', { headers })).ok());
  const passphrase = 'fixture-only-passphrase';
  assert((await context.request.post(base + '/api/admin/passphrase-status', { headers, data: { passphrase } })).ok());
  await context.addInitScript(pass => {
    if (window.top === window) sessionStorage.setItem('admin_passphrase', pass);
  }, passphrase);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(base + '/admin/#email-tools');
  await page.getByText('Connected to mitch-mail-rs.', { exact: true }).waitFor();
  const send = page.getByRole('button', { name: 'Send test email', exact: true });
  await page.waitForFunction(() => !document.getElementById('email-test-send').disabled);
  assert.equal(messages.length, 0, 'Opening the tools must not send email.');
  assert.equal(await page.locator('#email-preview').getAttribute('sandbox'), '');
  await page.frameLocator('#email-preview').getByText('Test email only.', { exact: false }).waitFor();
  for (const template of ['signup', 'login', 'reset', 'security']) {
    await page.selectOption('#email-template', template);
    await page.waitForFunction(() => !document.getElementById('email-test-send').disabled);
    assert.match(await page.locator('#email-subject').innerText(), /^\[Test\]/);
  }
  assert.equal(await page.locator('#email-test-recipient').inputValue(), 'admin@mitch.pro');
  await send.click();
  await page.getByText('SMTP accepted the test email for admin@mitch.pro', { exact: false }).waitFor();
  assert.equal(messages.length, 1);
  assert.equal(messages[0].to, 'admin@mitch.pro');
  assert.equal(messages[0].sender, 'noreply');
  assert.equal(messages[0].raw, true);
  assert.equal(messages[0].cc, undefined);
  assert.equal(messages[0].bcc, undefined);
  assert(messages[0].body.includes('Test email only.'));
  await page.locator('#email-test-recipient').fill('custom@example.com');
  rejectSend = true;
  await send.click();
  await page.getByText('mitch-mail-rs did not confirm SMTP acceptance.', { exact: false }).waitFor();
  assert.equal(messages.length, 2);
  assert.equal(messages[1].to, 'custom@example.com');
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `Overflow at ${width}px`);
  }
  if (process.env.EMAIL_SCREENSHOT) await page.screenshot({ path: process.env.EMAIL_SCREENSHOT, fullPage: true });
  assert.deepEqual(errors, []);
  console.log('PASS: real admin authentication, connection status, four sandboxed previews, dynamic-recipient test, SMTP rejection and responsive layout. All mail requests went to the local fake service.');
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  if (server.exitCode === null) await once(server, 'exit');
  await new Promise(resolve => mail.close(resolve));
  await rm(fixture, { recursive: true, force: true });
}
