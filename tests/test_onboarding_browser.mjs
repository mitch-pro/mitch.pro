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
  for (const path of ['/faq/', '/faq.html', '/faq/index.html', '/privacy/', '/privacy.html', '/use-agreement/', '/use-agreement.html', '/index-sales.html', '/onboarding.css?v=5', '/onboarding.js?v=5', '/enroll/enroll.css?v=1', '/media/site-tour-poster-v3.webp', '/rjuhsd-assets/pathways-logo.png', '/rjuhsd-assets/rosevilleadult-logo.png', '/rjuhsd-assets/district-logo.png']) {
    const response = await fetch(base + path);
    assert.equal(response.status, 200, `${path} should be public`);
    assert(new URL(response.url).pathname !== '/enroll/', `${path} must not redirect to login`);
  }
  const helpAlias = await fetch(base + '/faq/', { redirect: 'manual' });
  assert.equal(helpAlias.status, 302);
  assert.equal(helpAlias.headers.get('location'), '/#help');
  for (const path of ['/members/', '/vms/', '/api/vm/computers']) {
    const response = await fetch(base + path, { redirect: 'manual' });
    assert([302, 401, 403].includes(response.status), `${path} must remain protected`);
  }
  for (const version of [1, 2, 3, 4]) {
    const path = '/media/site-tour-v' + version + '.mp4';
    const range = await fetch(base + path, { headers: { Range: 'bytes=0-99' }, redirect: 'manual' });
    assert.equal(range.status, 206, path + ' must support seeking without a login');
    assert.match(range.headers.get('content-type'), /^video\/mp4/);
    assert.match(range.headers.get('content-range'), /^bytes 0-99\/\d+$/);
    assert.equal((await range.arrayBuffer()).byteLength, 100);
    const head = await fetch(base + path, { method: 'HEAD' });
    assert.equal(head.status, 200);
    assert.equal(head.headers.get('accept-ranges'), 'bytes');
    const suffix = await fetch(base + path, { headers: { Range: 'bytes=-50' } });
    assert.equal(suffix.status, 206);
    assert.equal((await suffix.arrayBuffer()).byteLength, 50);
    assert.equal((await fetch(base + path, { headers: { Range: 'bytes=999999999-' } })).status, 416);
  }

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
    for (const width of [320, 390, 768, 1024, 1440]) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 1000 });
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${name} must fit at ${width}px`);
      if (name === 'landing') {
        const player = await page.locator('.tour-stage').boundingBox();
        assert(player && player.y < 250, 'The video must lead the public intro at ' + width + 'px');
      }
      await page.screenshot({ path: `${out}/${name}-${width}.png`, fullPage: width === 1440 });
    }
  }
  await page.goto(base + '/');
  assert.equal(await page.locator('.tour-intro h1').innerText(), 'mitch.pro');
  assert.match(await page.locator('.tour-intro').innerText(), /Games, chat, and cloud Linux/);
  assert.equal(await page.locator('.school-card').count(), 8);
  assert.equal(await page.locator('[data-school-games-link]').getAttribute('href'), 'https://mitchdog.com/game-portal/');
  assert.equal(await page.locator('.school-site-link').getAttribute('href'), 'https://rjuhsd.school/');
  for (const mark of await page.locator('.school-card img').all()) {
    await mark.scrollIntoViewIfNeeded();
    await mark.evaluate(img => img.complete && img.naturalWidth > 0 || new Promise((resolve, reject) => { img.onload = resolve; img.onerror = reject; }));
    assert(await mark.evaluate(img => img.naturalWidth > 0), 'Every school logo must load');
  }
  assert.equal(await page.locator('.ob-campus-orbit,.ob-video-bar,.ob-eyebrow').count(), 0);
  assert.equal(await page.locator('#site-tour').getAttribute('preload'), 'metadata');
  assert.equal(await page.locator('#site-tour').getAttribute('autoplay'), null);
  assert.equal(await page.locator('#site-tour source').getAttribute('src'), '/media/site-tour-v4.mp4');
  assert.equal(await page.locator('#site-tour').getAttribute('poster'), '/media/site-tour-poster-v3.webp');
  await page.waitForFunction(() => Number.isFinite(document.getElementById('site-tour').duration));
  const duration = await page.locator('#site-tour').evaluate(video => video.duration);
  assert(Math.abs(duration - 237) < 0.01, 'The soundtrack edit must be exactly 3:57');
  assert.equal(await page.locator('#tour-duration').innerText(), '3:57');
  assert(await page.locator('#site-tour').evaluate(video => video.paused));
  await page.locator('#tour-play').scrollIntoViewIfNeeded();
  assert(await page.locator('#tour-play').isVisible());
  await page.locator('#tour-play').focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => {
    const video = document.getElementById('site-tour');
    return !video.paused && video.currentTime > 0.2 && video.webkitAudioDecodedByteCount > 0;
  });
  assert(await page.locator('#site-tour').evaluate(video => !video.muted && video.volume > 0));
  assert(await page.locator('#tour-play').isHidden());
  await page.locator('#site-tour').evaluate(video => video.pause());
  const chapters = await page.locator('[data-tour-time]').evaluateAll(buttons => buttons.map(button => Number(button.dataset.tourTime)));
  assert.deepEqual(chapters, [0,8.3,24.7,45.5,59.9,90.3,119.9,189.5,206.3,226.7]);
  for (const time of chapters) {
    await page.locator('[data-tour-time="' + time + '"]').click();
    await page.waitForFunction(time => {
      const video = document.getElementById('site-tour');
      return !video.paused && video.currentTime >= time && video.currentTime < time + 4 && video.readyState >= 2;
    }, time);
    await page.waitForFunction(time => document.querySelector('[data-tour-time="' + time + '"]').getAttribute('aria-current') === 'true', time);
    await page.locator('#site-tour').evaluate(video => video.pause());
  }
  assert(await page.locator('#site-tour').evaluate(video => video.videoWidth === 1920 && video.videoHeight === 968));
  assert.equal(await page.locator('#tour-status').innerText(), '');
  await page.screenshot({ path: out + '/tour-chapters.png' });
  await page.locator('#site-tour').evaluate(video => { video.currentTime = video.duration - 0.08; return video.play(); });
  await page.waitForFunction(() => document.getElementById('site-tour').ended);
  assert(await page.getByRole('button', {name:'Replay site tour with sound'}).isVisible());
  await page.getByRole('button', {name:'Replay site tour with sound'}).click();
  await page.waitForFunction(() => { const v=document.getElementById('site-tour'); return !v.paused && v.currentTime < 3; });
  await page.locator('#site-tour').evaluate(video => video.pause());
  await page.getByText('Do I have to pay for a VM?', { exact: true }).click();
  assert(await page.locator('.help-list details').first().getAttribute('open') !== null);

  await page.goto(base + '/enroll/?mode=signup');
  assert.equal(await page.locator('video,.ob-auth-story').count(), 0);
  assert.equal(await page.locator('.enroll-main').count(), 1);
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
  assert.equal(await page.locator('#login-2fa-label').innerText(), 'Authenticator code');
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
    for (const width of [320,390,768,1024,1440]) {
      await page.setViewportSize({width,height:width < 768 ? 844 : 1000});
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), name + ' must fit light mode at ' + width + 'px');
      if ([390,1440].includes(width)) await page.screenshot({path: out + '/' + name + '-light-' + width + '.png'});
    }
  }
  assert.deepEqual(errors, [], 'Pages must not throw JavaScript errors');
  console.log('PASS: public help and policy routes; protected account routes; video ranges/playback/chapters; responsive dark/light layouts; signup/verification/resend; login/2FA; reset; invite prefill; keyboard tabs; CSRF headers.');
  console.log(`Screenshots: ${out}`);
} finally {
  await browser?.close();
  server.kill('SIGTERM');
}
