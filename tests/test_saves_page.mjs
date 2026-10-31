// Save manager (/saves/) against a real local server and a fixture account.
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { createHash, createHmac } from 'node:crypto';
import { mkdtemp, readFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { createServer } from 'node:net';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const data = await mkdtemp('/tmp/mitch-saves-');
const socket = createServer();
await new Promise(r => socket.listen(0, '127.0.0.1', r));
const port = socket.address().port; await new Promise(r => socket.close(r));
const base = 'http://127.0.0.1:' + port;
const server = spawn(root + '/rust/target/debug/mitch-server', [], {
  cwd: root, stdio: 'ignore', env: { ...process.env, DATA_DIR: data, MITCH_BASE: root,
    USERDATA_DIR: data + '/userdata', NODE_ENV: 'test', GOD_MODE: '0', PORT: String(port), VAPID_PUBLIC_KEY: '', RECAPTCHA_SITE_KEY: '' }
});
const delay = ms => new Promise(r => setTimeout(r, ms));
let browser;
try {
  for (let i = 0; i < 150; i++) { try { if ((await fetch(base + '/enroll/')).ok) break; } catch {} await delay(100); }
  const secret = await readFile(data + '/id_secret.key');
  const db = new DatabaseSync(data + '/mitchpro.db');
  const email = 'saves-test@student.rjuhsd.us';
  const raw = 'e' + createHash('sha256').update(email).digest('hex').slice(0, 24);
  const sid = raw + '.' + createHmac('sha256', secret).update(raw).digest('hex').slice(0, 16);
  db.prepare('INSERT INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)').run(sid, email, email, Date.now());
  db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run(sid, JSON.stringify({ email, used: true }), Date.now());

  // Signed out: sign-in prompt, no data shown.
  browser = await chromium.launch();
  const anon = await browser.newContext();
  const ap = await anon.newPage();
  await ap.goto(base + '/saves/');
  await ap.waitForTimeout(800);
  // (an anonymous request may be redirected to enroll; either way no saves UI)
  assert.equal(await ap.locator('#main').isVisible().catch(() => false), false, 'no manager when signed out');

  const ctx = await browser.newContext({ viewport: { width: 1100, height: 900 } });
  await ctx.addCookies([{ name: 'studentId', value: sid, url: base }, { name: 'id', value: sid, url: base }]);
  const p = await ctx.newPage();
  const errors = [];
  p.on('pageerror', e => errors.push(e.message));
  p.on('dialog', d => d.accept());
  await p.goto(base + '/saves/');
  await p.evaluate(() => { localStorage.setItem('chess_elo', JSON.stringify({ elo: 1234, wins: 3 })); localStorage.setItem('_prefDisplay', JSON.stringify({ gameLaunch: 'tab' })); localStorage.setItem('cinny_access_token', 'secret-token'); localStorage.setItem('_e2e_private_jwk', '{"d":"secret"}'); });
  await p.reload();
  await p.locator('#main').waitFor({ state: 'visible' });
  assert.match(await p.locator('#st-last').innerText(), /Never/);
  assert.ok(await p.locator('.row', { hasText: 'chess_elo' }).count() >= 1, 'local item listed');
  assert.ok(await p.locator('.badge.local').count() >= 1, 'local-only badge');

  // Sync now → server has it, badges become Synced.
  await p.locator('#btn-sync').click();
  await p.locator('#status.ok').waitFor();
  const afterSync = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal(afterSync._snapshot.chess_elo.elo, 1234);
  assert.ok(afterSync._snapshot_ts > 0);
  // Matrix login and E2E keys never reach the server, even if a client posts them.
  assert.equal('cinny_access_token' in afterSync._snapshot, false);
  assert.equal('_e2e_private_jwk' in afterSync._snapshot, false);
  await p.evaluate(() => fetch('/api/userdata', { method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ _snapshot: { chess_elo: { elo: 1234, wins: 3 }, cinny_access_token: 'x', _e2e_key_history_v2: 'y' }, e2e_thing: 1 }) }));
  const posted = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.deepEqual(Object.keys(posted._snapshot), ['chess_elo']);
  assert.equal('e2e_thing' in posted, false);
  // The page neither lists them nor touches the local copy.
  assert.equal(await p.locator('.row', { hasText: 'cinny_access_token' }).count(), 0);
  assert.equal(await p.evaluate(() => localStorage.getItem('cinny_access_token')), 'secret-token');
  assert.equal(await p.locator('.badge.local').count(), 0);
  assert.ok(await p.locator('.badge.synced').count() >= 2);

  // Change locally → Differs; Restore pulls the server copy back.
  await p.evaluate(() => localStorage.setItem('chess_elo', JSON.stringify({ elo: 999 })));
  await p.reload();
  await p.locator('#main').waitFor({ state: 'visible' });
  const row = p.locator('.row', { hasText: 'chess_elo' }).first();
  assert.ok(await row.locator('.badge.differs').count() === 1, 'differs badge');
  await row.getByRole('button', { name: 'Restore' }).click();
  await p.locator('#status.ok').waitFor();
  assert.equal(JSON.parse(await p.evaluate(() => localStorage.getItem('chess_elo'))).elo, 1234);

  // Delete one entry from the server; the local copy stays.
  await row.getByRole('button', { name: 'Delete from server' }).click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Deleted'));
  const afterDel = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal('chess_elo' in afterDel._snapshot, false);
  assert.equal(await p.evaluate(() => !!localStorage.getItem('chess_elo')), true);

  // Top-level key delete + delete-all through the API the page uses.
  await p.evaluate(() => fetch('/api/userdata', { method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ game_slot: { a: 1 } }) }));
  await p.reload();
  await p.locator('.row', { hasText: 'game_slot' }).getByRole('button', { name: 'Delete from server' }).click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Deleted'));
  assert.equal(await p.evaluate(async () => 'game_slot' in (await (await fetch('/api/userdata', { credentials: 'include' })).json())), false);

  // Manual slot: separate from the auto slot, never touched by Sync now, sensitive keys stripped.
  assert.match(await p.locator('#m-ts').innerText(), /Empty/);
  assert.equal(await p.locator('#btn-m-restore').isDisabled(), true);
  await p.locator('#btn-m-save').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('manual slot'));
  const man = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal(man._snapshot2.chess_elo.elo, 1234);
  assert.ok(man._snapshot2_ts > 0);
  assert.equal('cinny_access_token' in man._snapshot2, false);
  assert.equal('_snapshot2' in man && Object.keys(man._snapshot2).some(k => k.startsWith('_sync')), false);
  await p.evaluate(() => localStorage.setItem('chess_elo', JSON.stringify({ elo: 5 })));
  await p.reload();
  await p.locator('#main').waitFor({ state: 'visible' });
  await p.locator('#btn-sync').click();
  await p.locator('#status.ok').waitFor();
  const both = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal(both._snapshot.chess_elo.elo, 5);
  assert.equal(both._snapshot2.chess_elo.elo, 1234, 'auto sync leaves the manual slot alone');
  assert.equal('_snapshot2' in both, true);
  assert.equal(await p.locator('.row', { hasText: '_snapshot2' }).count(), 0, 'slots are not listed as saved data');
  await p.locator('#btn-m-restore').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Restored'));
  assert.equal(JSON.parse(await p.evaluate(() => localStorage.getItem('chess_elo'))).elo, 1234);
  await p.locator('#btn-m-delete').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Deleted the manual'));
  const noMan = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal('_snapshot2' in noMan, false);
  assert.equal(noMan._snapshot.chess_elo.elo, 5, 'auto slot survives manual delete');

  await p.locator('#btn-sync').click();
  await p.locator('#status.ok').waitFor();
  await p.locator('#btn-delete-all').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Deleted all'));
  const wiped = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal(wiped._snapshot, undefined);

  // Game progress in IndexedDB (Emscripten IDBFS layout: out-of-line keys, an index, Date + Uint8Array values).
  const mkDb = (name) => p.evaluate((name) => new Promise((res, rej) => {
    const r = indexedDB.open(name, 21);
    r.onupgradeneeded = () => { const st = r.result.createObjectStore('FILE_DATA'); st.createIndex('timestamp', 'timestamp'); };
    r.onsuccess = () => { const db = r.result; const tx = db.transaction('FILE_DATA', 'readwrite'); tx.objectStore('FILE_DATA').put({ timestamp: new Date(1700000000000), mode: 33206, contents: new Uint8Array([1, 2, 3, 250]) }, '/sandbox/level.sav'); tx.oncomplete = () => { db.close(); res(); }; };
    r.onerror = () => rej(r.error);
  }), name);
  const readDb = () => p.evaluate(() => new Promise((res) => {
    const r = indexedDB.open('/sandbox');
    r.onsuccess = () => { const db = r.result; if (!db.objectStoreNames.contains('FILE_DATA')) { db.close(); return res(null); } const g = db.transaction('FILE_DATA').objectStore('FILE_DATA').get('/sandbox/level.sav'); g.onsuccess = () => { db.close(); const v = g.result; res(v && { ts: v.timestamp.getTime(), mode: v.mode, bytes: Array.from(v.contents), v: db.version }); }; };
  }));
  await mkDb('/sandbox');
  await mkDb('matrix-js-sdk:crypto');
  await p.reload();
  await p.locator('#main').waitFor({ state: 'visible' });
  assert.ok(await p.locator('.row', { hasText: '/sandbox' }).count() >= 1, 'game database listed');
  assert.equal(await p.locator('.row', { hasText: 'matrix-js-sdk' }).count(), 0, 'sensitive database not listed');
  await p.locator('#btn-m-save').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('manual slot'));
  const withIdb = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.deepEqual(Object.keys(withIdb._idb2), ['/sandbox'], 'only the game db reaches the manual slot');
  // Lose it locally (a different device), then restore from the manual slot.
  await p.evaluate(() => new Promise((res) => { const r = indexedDB.deleteDatabase('/sandbox'); r.onsuccess = res; r.onblocked = res; }));
  assert.equal((await p.evaluate(() => indexedDB.databases())).some(d => d.name === '/sandbox'), false);
  await p.reload();
  await p.locator('#main').waitFor({ state: 'visible' });
  await p.locator('#btn-m-restore').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Restored'));
  assert.deepEqual(await readDb(), { ts: 1700000000000, mode: 33206, bytes: [1, 2, 3, 250], v: 21 });
  await p.locator('#btn-m-delete').click();
  await p.waitForFunction(() => document.getElementById('status').textContent.includes('Deleted the manual'));
  assert.equal(await p.evaluate(async () => '_idb2' in (await (await fetch('/api/userdata', { credentials: 'include' })).json())), false);

  // Newer server copy: sync only notifies (never restores) and pauses saving until answered.
  await p.evaluate(() => fetch('/api/userdata', { method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ _snapshot: { chess_elo: { elo: 4321 } }, _snapshot_ts: Date.now() + 60000 }) }));
  await p.evaluate(() => { localStorage.setItem('chess_elo', JSON.stringify({ elo: 7 })); localStorage.setItem('_sync_ts', '0'); });
  await p.goto(base + '/preferences/');
  await p.locator('#_sync_restore_notice').waitFor({ timeout: 8000 });
  assert.equal(JSON.parse(await p.evaluate(() => localStorage.getItem('chess_elo'))).elo, 7, 'not auto-restored');
  await p.evaluate(() => window._mitchSync.save());
  const held = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json()));
  assert.equal(held._snapshot.chess_elo.elo, 4321, 'sync paused while the notice is open');
  await p.getByRole('button', { name: "Don't show again" }).click();
  assert.equal(JSON.parse(await p.evaluate(() => localStorage.getItem('_prefSync'))).restoreNotice, false);
  await p.waitForFunction(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json())._snapshot.chess_elo.elo === 7);
  await p.evaluate(() => window._mitchSync.saveIdb());
  const autoIdb = await p.evaluate(async () => (await (await fetch('/api/userdata', { credentials: 'include' })).json())._idb);
  assert.deepEqual(Object.keys(autoIdb), ['/sandbox'], 'auto-sync uploads game databases, not Matrix ones');
  await p.evaluate(() => localStorage.setItem('_sync_ts', '0'));
  await p.reload();
  await p.waitForTimeout(3000);
  assert.equal(await p.locator('#_sync_restore_notice').count(), 0, 'no notice once turned off');

  // Narrow viewport: no horizontal overflow.
  await p.setViewportSize({ width: 390, height: 800 });
  assert.equal(await p.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'overflow at 390');
  assert.deepEqual(errors, []);
  console.log('PASS: saves page sign-in gate, sync, differs/restore, per-entry delete, top-level delete, delete all, manual slot, IndexedDB game saves, restore notice, mobile layout.');
} finally {
  await browser?.close();
  server.kill();
}
