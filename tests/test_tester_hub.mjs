// Beta tester hub API (/api/tester/*) and the sandbox rules it controls,
// against a real local server and fixture accounts.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, createHmac } from 'node:crypto';
import { mkdir, mkdtemp, readFile, symlink, writeFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { createServer } from 'node:net';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const base = await mkdtemp('/tmp/mitch-tester-hub-');
await mkdir(base + '/data');
await symlink(root + '/webserver', base + '/webserver');
const tester = 'hubtester@student.rjuhsd.us';
const regular = 'hubregular@student.rjuhsd.us';
await writeFile(base + '/data/testers.json', JSON.stringify([tester]));

const socket = createServer();
await new Promise(r => socket.listen(0, '127.0.0.1', r));
const port = socket.address().port; await new Promise(r => socket.close(r));
const url = 'http://127.0.0.1:' + port;
const server = spawn(root + '/rust/target/debug/mitch-server', [], {
  cwd: base, stdio: 'ignore', env: { ...process.env, DATA_DIR: base + '/data', MITCH_BASE: base,
    USERDATA_DIR: base + '/userdata', NODE_ENV: 'test', GOD_MODE: '0', PORT: String(port), VAPID_PUBLIC_KEY: '', RECAPTCHA_SITE_KEY: '' }
});
const delay = ms => new Promise(r => setTimeout(r, ms));
try {
  for (let i = 0; i < 150; i++) { try { if ((await fetch(url + '/api/tester/status')).status === 403) break; } catch {} await delay(100); }
  const secret = await readFile(base + '/data/id_secret.key');
  const db = new DatabaseSync(base + '/data/mitchpro.db');
  const sidFor = email => {
    const raw = 'e' + createHash('sha256').update(email).digest('hex').slice(0, 24);
    const sid = raw + '.' + createHmac('sha256', secret).update(raw).digest('hex').slice(0, 16);
    db.prepare('INSERT INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)').run(sid, email, email, Date.now());
    db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run(sid, JSON.stringify({ email, used: true }), Date.now());
    return sid;
  };
  const sids = { tester: sidFor(tester), regular: sidFor(regular) };
  const call = async (who, method, path, body) => {
    const headers = { 'X-Mitch-Requested-With': '1' };
    if (who) headers.Cookie = `studentId=${sids[who]}; id=${sids[who]}`;
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const r = await fetch(url + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    let json = null; try { json = await r.json(); } catch {}
    return { status: r.status, json };
  };

  // Access: signed out 401, regular account 403, listed tester 200.
  const anon = await call(null, 'GET', '/api/tester/status');
  assert.ok([401, 403].includes(anon.status), 'signed out is refused');
  assert.equal((await call('regular', 'GET', '/api/tester/status')).status, 403);
  assert.equal((await call('regular', 'POST', '/api/tester/coins', { mode: 'unlimited' })).status, 403);
  let st = await call('tester', 'GET', '/api/tester/status');
  assert.equal(st.status, 200);
  assert.equal(st.json.isPremium, true);
  assert.equal(st.json.unlimitedCoins, true);
  assert.equal(st.json.coins, 999999999);
  assert.equal(st.json.settings.unlockAllCosmetics, false);

  // /api/me reflects the sandbox; the regular account is untouched.
  let wallet = await call('tester', 'GET', '/api/profile');
  assert.equal(wallet.json.unlimitedCoins, true);
  assert.equal(wallet.json.isTester, true);
  assert.equal(wallet.json.coins, 999999999);
  assert.equal((await call('tester', 'GET', '/api/me')).json.isPremium, true);
  wallet = await call('regular', 'GET', '/api/profile');
  assert.equal(wallet.json.unlimitedCoins, false);
  assert.equal(wallet.json.coins, 0);
  assert.equal((await call('regular', 'GET', '/api/me')).json.isPremium, false);

  // Coins: custom exact balance, validation, back to unlimited.
  assert.equal((await call('tester', 'POST', '/api/tester/coins', { mode: 'custom', amount: -5 })).status, 400);
  assert.equal((await call('tester', 'POST', '/api/tester/coins', { mode: 'custom', amount: 'x' })).status, 400);
  assert.equal((await call('tester', 'POST', '/api/tester/coins', { mode: 'nope' })).status, 400);
  let r = await call('tester', 'POST', '/api/tester/coins', { mode: 'custom', amount: 50 });
  assert.deepEqual([r.json.ok, r.json.unlimitedCoins, r.json.coins], [true, false, 50]);
  wallet = await call('tester', 'GET', '/api/profile');
  assert.equal(wallet.json.coins, 50);
  assert.equal(wallet.json.unlimitedCoins, false);
  r = await call('tester', 'POST', '/api/tester/coins', { mode: 'unlimited' });
  assert.equal(r.json.unlimitedCoins, true);

  // Premium toggle drives the site-wide premium check.
  r = await call('tester', 'POST', '/api/tester/toggle-premium', { premium: false });
  assert.equal(r.json.isPremium, false);
  assert.equal((await call('tester', 'GET', '/api/me')).json.isPremium, false);
  assert.equal((await call('tester', 'POST', '/api/tester/toggle-premium', {})).status, 400);
  await call('tester', 'POST', '/api/tester/toggle-premium', { premium: true });
  assert.equal((await call('tester', 'GET', '/api/me')).json.isPremium, true);

  // Settings toggles persist independently.
  r = await call('tester', 'POST', '/api/tester/settings', { unlockAllCosmetics: true });
  assert.equal(r.json.settings.unlockAllCosmetics, true);
  assert.equal(r.json.settings.bypassCooldowns, false);
  r = await call('tester', 'POST', '/api/tester/settings', { bypassCooldowns: true });
  assert.equal(r.json.settings.unlockAllCosmetics, true);
  assert.equal(r.json.settings.bypassCooldowns, true);

  // Unlock-all lets the tester equip a cosmetic they never bought.
  const eq = await call('tester', 'POST', '/api/me/cosmetics/equip', { type: 'name_color', itemId: 'neon_purple' });
  assert.equal(eq.status, 200, JSON.stringify(eq.json));
  await call('tester', 'POST', '/api/tester/settings', { unlockAllCosmetics: false });
  const eqReg = await call('regular', 'POST', '/api/me/cosmetics/equip', { type: 'name_color', itemId: 'neon_purple' });
  assert.equal(eqReg.status, 403, 'regular accounts still need to own it');

  // Daily claim: second claim blocked, bypass allows it, reset re-opens it.
  await call('tester', 'POST', '/api/tester/settings', { bypassCooldowns: false });
  assert.equal((await call('tester', 'POST', '/api/daily-login/claim', {})).status, 200);
  assert.equal((await call('tester', 'POST', '/api/daily-login/claim', {})).status, 400);
  await call('tester', 'POST', '/api/tester/settings', { bypassCooldowns: true });
  assert.equal((await call('tester', 'POST', '/api/daily-login/claim', {})).status, 200);
  await call('tester', 'POST', '/api/tester/settings', { bypassCooldowns: false });
  assert.equal((await call('tester', 'POST', '/api/daily-login/claim', {})).status, 400);
  assert.equal((await call('tester', 'POST', '/api/tester/reset-cooldowns')).json.ok, true);
  assert.equal((await call('tester', 'POST', '/api/daily-login/claim', {})).status, 200);

  // Sandbox coins can't reach real accounts: marketplace, wagers, gifting.
  assert.equal((await call('tester', 'POST', '/api/marketplace/buy', { listingId: 'x' })).status, 403);
  assert.equal((await call('tester', 'POST', '/api/marketplace/list', { type: 'text', price: 1 })).status, 403);
  const wager = await call('tester', 'POST', '/api/battleship/challenge', { to: regular, bet: 100 });
  assert.equal(wager.status, 400);
  assert.match(wager.json.error, /Tester accounts/);
  assert.equal((await call('tester', 'POST', '/api/coins/gift-premium', { targetEmail: regular })).status, 403);

  // Test notification reaches the account.
  assert.equal((await call('tester', 'POST', '/api/tester/test-notification', { message: '' })).status, 400);
  assert.equal((await call('tester', 'POST', '/api/tester/test-notification', { message: 'hello hub' })).json.ok, true);
  const notes = await call('tester', 'GET', '/api/me/notifications');
  assert.ok(JSON.stringify(notes.json).includes('hello hub'), 'notification delivered');

  // Leaderboard never lists the tester, whatever their balance.
  await call('tester', 'POST', '/api/tester/coins', { mode: 'custom', amount: 999999999 });
  const lb = await call('regular', 'GET', '/api/leaderboard');
  assert.equal(lb.status, 200);
  assert.ok(!JSON.stringify(lb.json).includes('hubtester'), 'tester not on leaderboard');

  console.log('PASS: tester hub access, coins sandbox, premium, cosmetics unlock, cooldowns, notification, wager/marketplace/gift blocks, leaderboard exclusion.');
} finally {
  server.kill();
}
