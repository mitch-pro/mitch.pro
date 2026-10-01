// Run after cargo build --manifest-path rust/Cargo.toml -p mitch-server.
// Uses isolated local data and mocked account requests; never production.
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { createServer } from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

const root = resolve(import.meta.dirname, '..');
const out = resolve(root, 'artifacts/ui-review/onboarding');
await mkdir(out, { recursive: true });
const data = await mkdtemp(resolve(tmpdir(), 'mitch-onboarding-'));
const socket = createServer();
await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
const port = socket.address().port;
await new Promise(resolve => socket.close(resolve));
const base = `http://127.0.0.1:${port}`;
const server = spawn(resolve(root, 'rust/target/debug/mitch-server'), [], {
  cwd: root, env: { ...process.env, NODE_ENV: 'test', DATA_DIR: data, MITCH_BASE: root, PORT: String(port), GOD_MODE: '0', RECAPTCHA_SITE_KEY: '' },
  stdio: 'ignore'
});
server.on('error', error => { console.error(error); });
let browser;
try {
  let ready = false;
  for (let attempt = 0; attempt < 80; attempt++) {
    try { ready = (await fetch(`${base}/enroll/`)).ok; } catch {}
    if (ready) break;
    await delay(150);
  }
  assert(ready, 'Build the local Rust server before running this test');
  for (const path of ['/faq/', '/faq.html', '/faq/index.html', '/privacy/', '/privacy.html', '/use-agreement/', '/use-agreement.html', '/onboarding.css?v=1', '/onboarding.js?v=1', '/media/site-tour-poster-v1.webp']) {
    const response = await fetch(base + path);
    assert.equal(response.status, 200, `${path} should be public`);
    assert(!response.url.includes('/enroll/'), `${path} must not redirect to login`);
  }
  for (const path of ['/members/', '/vms/', '/api/vm/computers']) {
    const response = await fetch(base + path, { redirect: 'manual' });
    assert([302, 401, 403].includes(response.status), `${path} must remain protected`);
  }
  const range = await fetch(base + '/media/site-tour-v1.mp4', { headers: { Range: 'bytes=0-99' } });
  assert.equal(range.status, 206);
  assert.match(range.headers.get('content-range'), /^bytes 0-99\/\d+$/);
  assert.equal((await range.arrayBuffer()).byteLength, 100);
  const head = await fetch(base + '/media/site-tour-v1.mp4', { method: 'HEAD' });
  assert.equal(head.status, 200);
  assert.equal(head.headers.get('accept-ranges'), 'bytes');
  const suffix = await fetch(base + '/media/site-tour-v1.mp4', { headers: { Range: 'bytes=-50' } });
  assert.equal(suffix.status, 206);
  assert.equal((await suffix.arrayBuffer()).byteLength, 50);
  assert.equal((await fetch(base + '/media/site-tour-v1.mp4', { headers: { Range: 'bytes=999999999-' } })).status, 416);

  browser = await chromium.launch({ channel: 'chromium', headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, serviceWorkers: 'block', reducedMotion: 'reduce' });
  const errors = [];
  const requests = [];
  let loginMode = 'error';
  let signupCount = 0;
  let verifyMode = false;
  const email = 'member@example.test';
  await context.addInitScript(() => {
    localStorage.setItem('_mitch_cookie_consent', 'accepted');
    window.getCaptchaToken = async () => 'local-test-captcha';
  });
  await context.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== base) {
      if (['fonts.googleapis.com', 'fonts.gstatic.com'].includes(url.hostname)) return route.continue();
      return route.abort();
    }
    if (!url.pathname.startsWith('/api/')) return route.continue();
    const req = route.request();
    if (req.method() === 'POST') requests.push({ path: url.pathname, body: req.postDataJSON(), csrf: req.headers()['x-mitch-requested-with'] });
    let status = 200, result = {};
    if (url.pathname === '/api/signup') { signupCount++; result = { success: true }; }
    else if (url.pathname === '/api/verify-signup') result = verifyMode ? { success: true, email } : { success: false, message: 'That code is not valid.' };
    else if (url.pathname === '/api/login') result = loginMode === '2fa' ? { twofa_required: true, temp_token: 'local-token', twofa_type: 'totp' } : { success: false, message: 'Check your email and password.' };
    else if (url.pathname === '/api/verify-2fa') result = { success: false, message: 'Check your authenticator code.' };
    else if (url.pathname === '/api/request-access') result = { success: true };
    else if (url.pathname === '/api/claim-token') result = { success: false, message: 'That reset code is not valid.' };
    else if (url.pathname === '/api/me') result = { email, rawEmail: email, normEmail: email };
    else if (url.pathname === '/api/bad-passwords') result = ['password123'];
    else if (url.pathname === '/api/dev/test-access') result = { enabled: false };
    else { status = 401; result = { error: 'Unauthenticated' }; }
    return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(result) });
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  for (const [name, path] of [['landing', '/'], ['signup', '/enroll/?mode=signup'], ['help', '/faq/']]) {
    await page.goto(base + path);
    await page.evaluate(() => document.fonts.ready);
    for (const width of [320, 390, 768, 1440]) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 1000 });
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${name} must fit at ${width}px`);
      await page.screenshot({ path: `${out}/${name}-${width}.png`, fullPage: width === 1440 });
    }
  }
  await page.goto(base + '/');
  assert.equal(await page.locator('#site-tour').getAttribute('preload'), 'none');
  await page.locator('[data-tour-time="20"]').click();
  await page.waitForFunction(() => { const v = document.getElementById('site-tour'); return !v.paused && v.currentTime >= 20 && v.readyState >= 2; });
  assert(await page.locator('#site-tour').evaluate(v => v.videoWidth > 0 && !v.muted));
  await page.locator('[data-tour-time="120"]').click();
  await page.waitForFunction(() => document.getElementById('site-tour').currentTime >= 120);
  await page.locator('#site-tour').evaluate(v => v.pause());
  await page.getByText('Do I have to pay for a VM?', { exact: true }).click();
  assert(await page.locator('.ob-faq details').first().getAttribute('open') !== null);

  await page.goto(base + '/enroll/?mode=signup');
  await page.clock.install();
  assert(await page.locator('#pane-invite').isVisible());
  assert(await page.locator('#btn').isDisabled());
  await page.locator('#email').fill('not-an-email');
  await page.locator('#signup-password').fill('a-strong-test-password');
  await page.locator('#signup-confirm-password').fill('a-strong-test-password');
  await page.locator('#agree').check();
  await page.locator('#btn').click();
  assert.match(await page.locator('#msg').innerText(), /valid email/);
  assert.equal(signupCount, 0);
  await page.locator('#email').fill(email);
  await page.locator('#signup-confirm-password').fill('different-password');
  await page.locator('#btn').click();
  assert.match(await page.locator('#msg').innerText(), /do not match/);
  assert.equal(signupCount, 0);
  await page.locator('#signup-confirm-password').fill('a-strong-test-password');
  const show = page.locator('[aria-controls="signup-password"]');
  await show.click(); assert.equal(await page.locator('#signup-password').getAttribute('type'), 'text');
  await show.click(); assert.equal(await page.locator('#signup-password').getAttribute('type'), 'password');
  await page.locator('#btn').click();
  await page.locator('#invite-step-2').waitFor({ state: 'visible' });
  assert.equal(signupCount, 1);
  assert(await page.locator('#resend-btn').isDisabled());
  await page.clock.fastForward(31000);
  assert(await page.locator('#resend-btn').isEnabled());
  await page.locator('#resend-btn').click();
  await page.waitForFunction(() => document.getElementById('resend-btn').textContent.startsWith('Resend available in'));
  await page.clock.fastForward(31000);
  assert(await page.locator('#resend-btn').isEnabled(), 'Resend must unlock again after a repeated request');
  await page.locator('#token').fill('123456');
  await page.locator('#tbtn').click();
  await page.waitForFunction(() => document.getElementById('tmsg').textContent.includes('not valid'));
  assert(await page.locator('#tbtn').isEnabled());
  await page.clock.resume();
  verifyMode = true;
  await page.locator('#tbtn').click();
  await page.waitForURL(base + '/');

  await page.goto(base + '/enroll/');
  await page.locator('#login-email').fill(email);
  await page.locator('#login-password').fill('test-password');
  await page.locator('#login-password').press('Enter');
  await page.waitForFunction(() => document.getElementById('login-msg').textContent.includes('Check your email'));
  assert(await page.locator('#login-btn').isEnabled());
  loginMode = '2fa';
  await page.locator('#login-btn').click();
  await page.locator('#login-2fa-panel').waitFor({ state: 'visible' });
  assert.equal(await page.locator('#login-2fa-label').innerText(), 'Authenticator Code');
  await page.locator('#login-2fa-code').fill('654321');
  await page.locator('#login-2fa-btn').click();
  await page.waitForFunction(() => document.getElementById('login-msg').textContent.includes('Check your authenticator'));
  await page.locator('#tab-login').focus();
  await page.keyboard.press('ArrowRight');
  assert(await page.locator('#pane-invite').isVisible());
  assert.equal(await page.locator('#tab-invite').getAttribute('tabindex'), '0');
  await page.keyboard.press('End');
  assert(await page.locator('#pane-reset').isVisible());
  await page.locator('#reset-email').fill(email);
  await page.locator('#reset-request-btn').click();
  await page.locator('#reset-step-2').waitFor({ state: 'visible' });
  await page.locator('#reset-token').fill('123456');
  await page.locator('#reset-password').fill('new-test-password');
  await page.locator('#reset-confirm-password').fill('new-test-password');
  await page.locator('#rbtn').click();
  await page.waitForFunction(() => document.getElementById('rtmsg').textContent.includes('not valid'));
  await page.goto(base + '/enroll/?reset=654321');
  assert.equal(await page.locator('#reset-token').inputValue(), '654321');
  await page.goto(base + '/enroll/?ref=FRIEND&email=invited%40example.test&mode=signup');
  assert.equal(await page.locator('#email').inputValue(), 'invited@example.test');
  assert.match(await page.locator('.invite-banner').innerText(), /FRIEND/);
  for (const request of requests) assert.equal(request.csrf, '1', `${request.path} must keep its CSRF header`);
  assert.equal(requests.find(r => r.path === '/api/signup').body.email, email);
  assert.equal(await page.evaluate(() => { history.replaceState(null, '', '?next=https://evil.example'); return postLoginDestination(); }), '/');
  await context.addCookies([{ name: 'theme', value: 'light', url: base }]);
  for (const [name, path] of [['landing', '/'], ['signup', '/enroll/?mode=signup'], ['help', '/faq/']]) {
    await page.goto(base + path);
    assert(await page.locator('html').evaluate(h => h.classList.contains('theme-light')));
    await page.screenshot({ path: `${out}/${name}-light.png`, fullPage: false });
  }
  assert.deepEqual(errors, [], 'Pages must not throw JavaScript errors');
  console.log('PASS: public help and policy routes; protected account routes; video ranges/playback/chapters; responsive dark/light layouts; signup/verification/resend; login/2FA; reset; invite prefill; keyboard tabs; CSRF headers.');
  console.log(`Screenshots: ${out}`);
} finally {
  await browser?.close();
  server.kill('SIGTERM');
}
