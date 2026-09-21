import assert from 'node:assert/strict';
import './test_matrix_word_filter.js';
import { join } from 'node:path';
import { readFileSync, existsSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHash, createHmac, randomBytes } from 'node:crypto';
import { configureDataStore, readDocument, writeDocument } from '../lib/data_store.js';

const REPO_ROOT = join(import.meta.dir, '..');
const DATA_DIR = join(REPO_ROOT, 'data');
configureDataStore({ baseDir: REPO_ROOT, dataDir: DATA_DIR });

const ID_SECRET_FILE = join(DATA_DIR, 'id_secret.key');
let ID_SECRET;
try {
  ID_SECRET = readFileSync(ID_SECRET_FILE);
} catch {
  if (!existsSync(DATA_DIR)) {
    mkdirSync(DATA_DIR, { recursive: true });
  }
  ID_SECRET = randomBytes(32);
  writeFileSync(ID_SECRET_FILE, ID_SECRET);
}

const NAMES_FILE = join(DATA_DIR, 'names.json');
const PROFILES_FILE = join(DATA_DIR, 'profiles.json');
const PASSWORDS_FILE = join(DATA_DIR, 'passwords.json');
const MODERATORS_FILE = join(DATA_DIR, 'moderators.json');
const CHAT_REPORTS_FILE = join(DATA_DIR, 'chat_reports.json');
const PUSH_SUBS_FILE = join(DATA_DIR, 'push_subs.json');
const GENERATIONS_FILE = join(DATA_DIR, 'generations.json');

function normalizeEmail(email) {
  if (!email) return '';
  let e = String(email).toLowerCase().trim();
  if (!e.includes('@')) return e;
  const at = e.lastIndexOf('@');
  const localRaw = e.slice(0, at).split('+')[0];
  const domainRaw = e.slice(at + 1);
  const local = localRaw.replace(/\./g, '');
  const reservedMitchPro = new Set(['admin', 'support', 'noreply', 'mitch']);
  const domain = ((domainRaw === 'student.mitch.pro' || domainRaw === 'mitch.pro') && !reservedMitchPro.has(local))
    ? 'student.rjuhsd.us'
    : domainRaw;
  return local + '@' + domain;
}

function makeEmailId(email, gen = 0) {
  const key = gen === 0 ? email : `${email}:v${gen}`;
  const emailHash = createHash('sha256').update(key).digest('hex').slice(0, 24);
  const raw = 'e' + emailHash;
  const sig = createHmac('sha256', ID_SECRET).update(raw).digest('hex').slice(0, 16);
  return raw + '.' + sig;
}

function getMatrixPasswordForUid(uid) {
  const secret = ID_SECRET || 'mitch-matrix-secret-salt-2026';
  return createHmac('sha256', secret).update('matrix-account:' + uid).digest('hex');
}

function getGenFor(email) {
  const rec = (readDocument(GENERATIONS_FILE, {}) || {})[email];
  return (rec && typeof rec === 'object') ? (rec.gen || 0) : (rec || 0);
}

// 1. Regular test user
const testEmail = 'matrix_test_user@student.rjuhsd.us';
const testNormEmail = normalizeEmail(testEmail);
const testSid = makeEmailId(testNormEmail, getGenFor(testNormEmail));

// 2. Admin test user (in test mode, admin@mitch.pro is automatically recognized as admin)
const adminEmail = 'admin@mitch.pro';
const adminNormEmail = normalizeEmail(adminEmail);
const adminSid = makeEmailId(adminNormEmail, getGenFor(adminNormEmail));

// 3. Moderator test user
const modEmail = 'matrix_mod_user@student.rjuhsd.us';
const modNormEmail = normalizeEmail(modEmail);
const modSid = makeEmailId(modNormEmail, getGenFor(modNormEmail));

// 4. Dotted enrollment user for canonical email testing
const dottedEmail = 'mitchell.fogler@student.rjuhsd.us';
const dottedNormEmail = normalizeEmail(dottedEmail);
const dottedSid = makeEmailId(dottedNormEmail, getGenFor(dottedNormEmail));

// Ensure test users exist in names.json and profiles.json
const names = { ...readDocument(NAMES_FILE, {}) };
names[testSid] = testEmail;
names[adminSid] = adminEmail;
names[modSid] = modEmail;
names[dottedSid] = dottedEmail;
writeDocument(NAMES_FILE, names);

const profiles = { ...readDocument(PROFILES_FILE, {}) };
profiles[testNormEmail] = { username: 'matrixtestuser', displayName: 'Matrix Test User' };
profiles[adminNormEmail] = { username: 'admin', displayName: 'Site Administrator' };
profiles[modNormEmail] = { username: 'matrixmoduser', displayName: 'Matrix Mod User' };
writeDocument(PROFILES_FILE, profiles);

const origPasswords = readDocument(PASSWORDS_FILE, {});
const origPushSubs = readDocument(PUSH_SUBS_FILE, {});
const passwords = { ...origPasswords };
passwords[testNormEmail] = await Bun.password.hash('mitch_test_pass_123');
passwords[adminNormEmail] = await Bun.password.hash('admin_test_pass_123');
writeDocument(PASSWORDS_FILE, passwords);

// Ensure moderator user is in moderators.json
const MATRIX_NOTIFS_FILE = join(DATA_DIR, 'matrix_notifications.json');
const MATRIX_EMAIL_SENT_FILE = join(DATA_DIR, 'matrix_email_sent.json');
const MATRIX_ROOM_SETTINGS_FILE = join(DATA_DIR, 'matrix_room_settings.json');
const origNotifs = readDocument(MATRIX_NOTIFS_FILE, {});
const origEmailSent = readDocument(MATRIX_EMAIL_SENT_FILE, {});
const origRoomSettings = readDocument(MATRIX_ROOM_SETTINGS_FILE, {});
const origMods = readDocument(MODERATORS_FILE, []);
const origReports = readDocument(CHAT_REPORTS_FILE, []);
writeDocument(MATRIX_EMAIL_SENT_FILE, {});
writeDocument(MATRIX_ROOM_SETTINGS_FILE, {});
const mods = Array.from(new Set([...origMods, modNormEmail]));
writeDocument(MODERATORS_FILE, mods);

// Start Mock Conduit Server
const MOCK_CONDUIT_PORT = 6188;
let mockPowerLevels = { users: { '@mitch_admin:mitch.pro': 100 }, users_default: 0 };
const kickedUsers = [];
const bannedUsers = [];
const redactedEvents = [];
const matrixLoginBodies = [];
let mockDevices = [
  { device_id: 'DEV_CURRENT', last_seen_ts: Date.now() },
  { device_id: 'DEV_OLD_1', last_seen_ts: Date.now() - (10 * 24 * 60 * 60 * 1000) },
  { device_id: 'DEV_OLD_2', last_seen_ts: 0 }
];
const deletedDeviceIds = [];

const mockConduit = Bun.serve({
  port: MOCK_CONDUIT_PORT,
  async fetch(req) {
    const url = new URL(req.url);
    const path = url.pathname;
    const method = req.method;

    if (path === '/_matrix/client/v3/login' && method === 'POST') {
      const b = await req.json().catch(() => ({}));
      matrixLoginBodies.push(b);
      const user = b.identifier?.user || 'user';
      const expectedPassword = getMatrixPasswordForUid(testSid);
      if (b.type === 'm.login.password' && user === 'matrixtestuser' && b.password !== expectedPassword) {
        return Response.json({ errcode: 'M_FORBIDDEN', error: 'Invalid password' }, { status: 403 });
      }
      return Response.json({
        user_id: `@${user}:mitch.pro`,
        access_token: `tok_${user}`,
        device_id: b.device_id || 'DEV_MOCK',
        home_server: 'mitch.pro'
      });
    }
    if ((path === '/_matrix/client/v3/keys/device_signing/upload' || path === '/_matrix/client/v3/room_keys/version') && method === 'POST') {
      const b = await req.json().catch(() => ({}));
      const auth = b.auth || {};
      const expectedPassword = getMatrixPasswordForUid(testSid);
      if (auth.type === 'm.login.password' && auth.password === expectedPassword) {
        return Response.json({ ok: true, uia_authenticated: true });
      }
      return Response.json({ errcode: 'M_FORBIDDEN', error: 'Invalid password' }, { status: 403 });
    }
    if (path === '/_matrix/client/v3/account/whoami') {
      return Response.json({ user_id: '@matrixtestuser:mitch.pro', device_id: 'DEV_MOCK' });
    }
    if (path === '/_matrix/client/v3/register' && method === 'POST') {
      const b = await req.json().catch(() => ({}));
      const user = b.username || 'user';
      return Response.json({ user_id: `@${user}:mitch.pro`, access_token: `tok_${user}`, device_id: 'DEV_MOCK', home_server: 'mitch.pro' });
    }
    if (path.startsWith('/_matrix/client/v3/profile/')) {
      return Response.json({});
    }
    if (path.startsWith('/_matrix/client/v3/directory/room/')) {
      return Response.json({ room_id: '!official_general:mitch.pro' });
    }
    if (path === '/_matrix/client/v3/createRoom') {
      return Response.json({ room_id: '!official_general:mitch.pro' });
    }
    if (path.startsWith('/_matrix/client/v3/join/')) {
      return Response.json({ room_id: '!official_general:mitch.pro' });
    }
    if (path.includes('/state/m.room.power_levels')) {
      if (method === 'GET') {
        return Response.json(mockPowerLevels);
      }
      if (method === 'PUT') {
        mockPowerLevels = await req.json();
        return Response.json({ event_id: '$pl_upd_' + Date.now() });
      }
    }
    if (path.includes('/event/')) {
      return Response.json({
        event_id: path.split('/').pop(),
        sender: '@test_spammer:mitch.pro',
        content: { body: 'This is a test inappropriate message!' },
        origin_server_ts: 1725900000000
      });
    }
    if (path.includes('/report/')) {
      return Response.json({});
    }
    if (path.endsWith('/kick')) {
      const b = await req.json().catch(() => ({}));
      kickedUsers.push(b.user_id);
      return Response.json({});
    }
    if (path.endsWith('/ban')) {
      const b = await req.json().catch(() => ({}));
      bannedUsers.push(b.user_id);
      return Response.json({});
    }
    if (path.includes('/redact/')) {
      redactedEvents.push(decodeURIComponent(path));
      return Response.json({ event_id: '$redacted_' + Date.now() });
    }
    if (path.includes('/joined_members')) {
      return Response.json({
        joined: {
          '@matrixtestuser:mitch.pro': {},
          '@admin:mitch.pro': {}
        }
      });
    }
    if (path.includes('/state/m.room.name')) {
      return Response.json({ name: 'General Chat' });
    }
    if (path.includes('/state/m.room.canonical_alias')) {
      return Response.json({ alias: '#general:mitch.pro' });
    }
    if (path.includes('/send/')) {
      return Response.json({ event_id: '$ev_msg_' + Date.now() });
    }
    if (path.endsWith('/invite')) {
      return Response.json({});
    }
    if (path === '/_matrix/client/v3/devices') {
      return Response.json({ devices: mockDevices });
    }
    if (path === '/_matrix/client/v3/delete_devices' && method === 'POST') {
      const b = await req.json().catch(() => ({}));
      const toDel = new Set(b.devices || []);
      mockDevices = mockDevices.filter(d => !toDel.has(d.device_id));
      deletedDeviceIds.push(...toDel);
      return Response.json({});
    }
    return Response.json({ error: 'not found' }, { status: 404 });
  }
});

// Start a test server instance
const TEST_PORT = 6855;
process.env.PORT = String(TEST_PORT);
process.env.NODE_ENV = 'test';
process.env.SESSION_COOKIE_SECURE = '0';
process.env.CONDUIT_PORT = String(MOCK_CONDUIT_PORT);
process.env.CONDUIT_HOST = '127.0.0.1';

console.log(`--- Starting server for Matrix SSO unit tests on port ${TEST_PORT} (mock Conduit port ${MOCK_CONDUIT_PORT}) ---`);
const serverProc = Bun.spawn(['bun', 'server.js'], {
  cwd: REPO_ROOT,
  env: {
    ...process.env,
    PORT: String(TEST_PORT),
    NODE_ENV: 'test',
    SESSION_COOKIE_SECURE: '0',
    PVE_SSH_HOST: '',
    DATA_DIR: DATA_DIR,
    CONDUIT_PORT: String(MOCK_CONDUIT_PORT),
    CONDUIT_HOST: '127.0.0.1',
  },
  stdout: 'inherit',
  stderr: 'inherit'
});

async function waitForServer(port, maxAttempts = 40) {
  for (let i = 0; i < maxAttempts; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/api/site-info`);
      if (res.ok) return;
    } catch {}
    await Bun.sleep(400);
  }
  throw new Error(`Server failed to start on port ${port}`);
}

try {
  await waitForServer(TEST_PORT);
  const BASE_URL = `http://127.0.0.1:${TEST_PORT}`;

  // 1. Unauthenticated SSO status
  console.log('--- 1. Testing unauthenticated /api/matrix/sso-status ---');
  const resAnonStatus = await fetch(`${BASE_URL}/api/matrix/sso-status`);
  assert.equal(resAnonStatus.status, 200);
  const dataAnonStatus = await resAnonStatus.json();
  assert.equal(dataAnonStatus.authenticated, false, 'Unauthenticated user should report authenticated: false');
  console.log('Unauthenticated status passed');

  // 2. Unauthenticated SSO login
  console.log('--- 2. Testing unauthenticated /api/matrix/sso-login ---');
  const resAnonLogin = await fetch(`${BASE_URL}/api/matrix/sso-login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' }
  });
  assert.equal(resAnonLogin.status, 401, 'Unauthenticated request to sso-login must return 401');
  const dataAnonLogin = await resAnonLogin.json();
  assert.equal(dataAnonLogin.ok, false);
  console.log('Unauthenticated login rejected with 401 as expected');

  // 3. Authenticated SSO status
  console.log('--- 3. Testing authenticated /api/matrix/sso-status ---');
  const resAuthStatus = await fetch(`${BASE_URL}/api/matrix/sso-status`, {
    headers: { 'Cookie': `studentId=${testSid}` }
  });
  assert.equal(resAuthStatus.status, 200);
  const dataAuthStatus = await resAuthStatus.json();
  assert.equal(dataAuthStatus.authenticated, true);
  assert.equal(dataAuthStatus.username, 'matrixtestuser');
  assert.equal(dataAuthStatus.user_id, '@matrixtestuser:mitch.pro');
  assert.equal(dataAuthStatus.displayName, 'Matrix Test User');
  console.log('Authenticated SSO status passed:', dataAuthStatus);

  console.log('--- 3b. Testing Matrix SSO returns to the school host that opened chat ---');
  const siteConfig = JSON.parse(readFileSync(join(DATA_DIR, 'site.json'), 'utf8'));
  const identityOrigin = new URL(siteConfig.alternate || 'https://mitchdog.com').origin;
  const identityHost = new URL(identityOrigin).host;
  const schoolMatrixBack = 'https://rjuhsd.school/matrix/';

  // Identity host is signed in; school Matrix is the destination. Exchange must
  // land back on rjuhsd.school — not rewrite the user onto mitchdog.com/matrix/.
  const bridgeRes = await fetch(`${BASE_URL}/api/sso/bridge?back=${encodeURIComponent(schoolMatrixBack)}`, {
    headers: {
      'Host': identityHost,
      'Cookie': `studentId=${testSid}`
    },
    redirect: 'manual'
  });
  assert.equal(bridgeRes.status, 302);
  const bridgeLocation = new URL(bridgeRes.headers.get('Location'));
  assert.equal(bridgeLocation.origin, 'https://rjuhsd.school', 'Matrix SSO exchange must target the school host that opened chat');
  assert.equal(bridgeLocation.pathname, '/api/sso/exchange');
  assert.equal(bridgeRes.headers.get('Referrer-Policy'), 'no-referrer');
  const bridgeToken = bridgeLocation.searchParams.get('token');
  assert(bridgeToken, 'Bridge handoff must contain a single-use token');
  const bridgeBack = bridgeLocation.searchParams.get('back');
  assert.equal(bridgeBack, schoolMatrixBack, 'Matrix SSO back URL must remain the school Matrix URL');

  const exchangeRes = await fetch(`${BASE_URL}/api/sso/exchange?${new URLSearchParams({
    token: bridgeToken,
    back: bridgeBack
  })}`, {
    headers: {
      'Host': 'rjuhsd.school'
    },
    redirect: 'manual'
  });
  assert.equal(exchangeRes.status, 302);
  assert.equal(exchangeRes.headers.get('Location'), schoolMatrixBack);
  assert(exchangeRes.headers.get('Set-Cookie')?.includes('mitch_session='), 'School exchange must create a session on rjuhsd.school');
  console.log('Matrix SSO return-to-school handoff passed');

  // Same-origin Matrix on the school host must not bounce to the alternate.
  const sameOriginBridge = await fetch(`${BASE_URL}/api/sso/bridge?back=${encodeURIComponent(schoolMatrixBack)}`, {
    headers: {
      'Host': 'rjuhsd.school',
      'Cookie': `studentId=${testSid}`
    },
    redirect: 'manual'
  });
  assert.equal(sameOriginBridge.status, 302);
  assert.equal(sameOriginBridge.headers.get('Location'), schoolMatrixBack, 'Signed-in school Matrix must stay on rjuhsd.school');
  console.log('Matrix SSO same-origin school stay passed');

  console.log('--- 3c. Testing Games SSO avoids the inline-script bridge page ---');
  const gameIdentityHost = new URL(siteConfig.primary || 'https://mitchdog.com').host;
  for (const gamePath of ['/games/', '/game-portal/', '/msn-games/']) {
    const gameBack = 'https://mitch.pro' + gamePath;
    const gameBridgeRes = await fetch(`${BASE_URL}/api/sso/bridge?back=${encodeURIComponent(gameBack)}`, {
      headers: {
        'Host': gameIdentityHost,
        'Cookie': `studentId=${testSid}`
      },
      redirect: 'manual'
    });
    assert.equal(gameBridgeRes.status, 302, `${gamePath} must redirect instead of rendering the Signing in page`);
    const gameLocation = new URL(gameBridgeRes.headers.get('Location'));
    assert.equal(gameLocation.origin, 'https://mitch.pro');
    assert.equal(gameLocation.pathname, '/api/sso/exchange');
    assert.equal(gameLocation.searchParams.get('back'), gameBack);
    assert(gameLocation.searchParams.get('token'), `${gamePath} handoff must include a single-use token`);
    assert.equal(gameBridgeRes.headers.get('Referrer-Policy'), 'no-referrer');
  }
  console.log('Games SSO redirect handoff passed');

  console.log('--- 3d. Testing homepage Play Games stays on the canonical host ---');
  const canonicalGamePortalRes = await fetch(`${BASE_URL}/game-portal/`, {
    headers: {
      'Host': gameIdentityHost,
      'Cookie': `studentId=${testSid}`
    },
    redirect: 'manual'
  });
  assert.equal(canonicalGamePortalRes.status, 200,
    'The canonical homepage game button must render the portal without bouncing to the legacy host');
  const canonicalGamePortalHtml = await canonicalGamePortalRes.text();
  assert(canonicalGamePortalHtml.includes('id="game-grid"'), 'Homepage game button must load the game portal');
  console.log('Homepage Play Games canonical-host flow passed');

  console.log('--- 3e. Testing non-Matrix SSO hop uses same-origin handoff (CSP form-action safe) ---');
  const pickleBack = 'https://sexypickleclub.com/';
  const pickleHopRes = await fetch(`${BASE_URL}/api/sso/bridge?back=${encodeURIComponent(pickleBack)}`, {
    headers: {
      'Host': 'mitch.pro',
      'Cookie': `studentId=${testSid}`
    },
    redirect: 'manual'
  });
  assert.equal(pickleHopRes.status, 200, 'Cross-domain non-Matrix SSO must return hop HTML');
  const pickleHopHtml = await pickleHopRes.text();
  assert(pickleHopHtml.includes('f.action = "/api/sso/bridge/handoff"') || pickleHopHtml.includes("f.action = \"/api/sso/bridge/handoff\""),
    'Hop page must same-origin POST to /api/sso/bridge/handoff (not cross-origin exchange)');
  assert(!/f\.action = "https:\/\/sexypickleclub\.com\/api\/sso\/exchange/.test(pickleHopHtml),
    'Hop page must not form-POST cross-origin (blocked by form-action \'self\')');
  const hopTokenMatch = pickleHopHtml.match(/add\("token", "([^"]+)"\)/);
  assert(hopTokenMatch, 'Hop HTML must embed the bridge token');
  const hopToken = hopTokenMatch[1];

  const sampleJwk = {
    kty: 'EC', crv: 'P-256',
    x: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
    y: 'BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB',
    d: 'CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC',
    ext: true
  };
  const handoffRes = await fetch(`${BASE_URL}/api/sso/bridge/handoff`, {
    method: 'POST',
    headers: {
      'Host': 'mitch.pro',
      'Content-Type': 'application/x-www-form-urlencoded'
    },
    body: new URLSearchParams({
      token: hopToken,
      back: pickleBack,
      e2ePrivateJwk: JSON.stringify(sampleJwk)
    }).toString(),
    redirect: 'manual'
  });
  assert.equal(handoffRes.status, 200, 'Handoff must finish same-origin so form-action does not block it');
  assert.equal(handoffRes.headers.get('Location'), null, 'Form handoff must not issue a cross-origin HTTP redirect');
  const handoffHtml = await handoffRes.text();
  const handoffLocationMatch = handoffHtml.match(/location\.replace\(("(?:[^"\\]|\\.)*")\)/);
  assert(handoffLocationMatch, 'Handoff HTML must continue with a top-level navigation');
  const handoffLoc = new URL(JSON.parse(handoffLocationMatch[1]));
  assert.equal(handoffLoc.origin, 'https://sexypickleclub.com');
  assert.equal(handoffLoc.pathname, '/api/sso/exchange');
  assert.equal(handoffLoc.searchParams.get('token'), hopToken);
  assert.equal(handoffLoc.searchParams.get('back'), pickleBack);

  const pickleExchangeRes = await fetch(`${BASE_URL}/api/sso/exchange?${handoffLoc.searchParams.toString()}`, {
    headers: { 'Host': 'sexypickleclub.com' },
    redirect: 'manual'
  });
  assert.equal(pickleExchangeRes.status, 200, 'Exchange with attached JWK must return settle HTML');
  assert(pickleExchangeRes.headers.get('Set-Cookie')?.includes('mitch_session='), 'Pickle exchange must create a session');
  const settleHtml = await pickleExchangeRes.text();
  assert(settleHtml.includes('localStorage.setItem'), 'Settle page must write E2E JWK into destination localStorage');
  assert(settleHtml.includes('sexypickleclub.com/'), 'Settle page must continue to pickle back URL');
  console.log('Pickle SSO same-origin handoff passed');

  // 4. Matrix config check
  console.log('--- 4. Testing /matrix/config.json ---');
  const resConfig = await fetch(`${BASE_URL}/matrix/config.json`);
  assert.equal(resConfig.status, 200);
  const configData = await resConfig.json();
  assert.equal(configData.defaultHomeserver, 0);
  assert(Array.isArray(configData.homeserverList));
  assert.deepEqual(configData.homeserverList, ['mitchdog.com'], 'Matrix must use only the canonical homeserver endpoint');
  assert.equal(configData.allowCustomHomeservers, false, 'custom homeservers must remain disabled');
  assert(configData.featuredCommunities);
  assert.equal(configData.featuredCommunities.openAsDefault, true);
  assert(configData.featuredCommunities.servers.includes('mitch.pro'));
  assert(configData.featuredCommunities.rooms.includes('#general:mitch.pro'));
  const matrixPage = readFileSync(join(REPO_ROOT, 'webserver', 'matrix', 'index.html'), 'utf8');
  assert(matrixPage.includes('storedSessionIsValid(stored.token, stored.userId, stored.deviceId)'), 'Matrix must validate both the user and device before reusing a browser session');
  assert(matrixPage.includes("body: JSON.stringify(reusableDeviceId ? { device_id: reusableDeviceId } : {})"), 'Matrix SSO refresh must request the browser\'s existing device ID');
  assert(matrixPage.includes("account in the store doesn't match the account in the constructor"), 'Matrix must detect the Rust crypto-store account mismatch');
  assert(matrixPage.includes('completePendingMatrixStoreRecovery()'), 'Matrix must repair mismatched IndexedDB stores before restarting Cinny');
  assert(matrixPage.includes("'matrix-js-sdk::matrix-sdk-crypto'"), 'Matrix recovery must clear the Rust crypto database that contains the mismatched account');
  assert(matrixPage.includes("navigator.locks.request('mitch-matrix-session'"), 'Concurrent tabs must serialize Matrix SSO');
  assert(!matrixPage.includes('removeLegacyCryptoStorage'), 'Matrix must preserve crypto storage for E2EE keys');
  assert(matrixPage.includes('/matrix/assets/index-BVlPv2dR.js'), 'Matrix bundle URL must load updated E2EE client');
  assert(!matrixPage.includes('__MATRIX_SSO_TARGET__'), 'Matrix page must not depend on __MATRIX_SSO_TARGET__ redirect injection');

  const resMatrixHtml = await fetch(`${BASE_URL}/matrix/`);
  assert.equal(resMatrixHtml.status, 200);
  const htmlContent = await resMatrixHtml.text();
  assert(!htmlContent.includes('__MATRIX_SSO_TARGET__'), 'Served /matrix/ HTML must not inject alternate-host redirect target');
  assert(htmlContent.includes('/api/sso/bridge?back=/matrix/') || htmlContent.includes('/api/sso/bridge?back=%2Fmatrix%2F'),
    'Served /matrix/ HTML must keep same-host SSO bridge back=/matrix/');
  console.log('/matrix/config.json and stay-on-origin HTML passed');

  // 4b. Matrix asset immutable caching and gzip serving check
  console.log('--- 4b. Testing asset caching and gzip for /matrix/assets ---');
  const resAssetNoGzip = await fetch(`${BASE_URL}/matrix/assets/index-BVlPv2dR.js`);
  assert.equal(resAssetNoGzip.status, 200);
  assert.equal(resAssetNoGzip.headers.get('Cache-Control'), 'public, max-age=31536000, immutable');
  assert.equal(resAssetNoGzip.headers.get('Content-Type'), 'application/javascript; charset=utf-8');

  const resAssetGzip = await fetch(`${BASE_URL}/matrix/assets/index-BVlPv2dR.js`, {
    headers: { 'Accept-Encoding': 'gzip' }
  });
  assert.equal(resAssetGzip.status, 200);
  assert.equal(resAssetGzip.headers.get('Content-Encoding'), 'gzip');
  assert.equal(resAssetGzip.headers.get('Cache-Control'), 'public, max-age=31536000, immutable');
  assert.equal(resAssetGzip.headers.get('Content-Type'), 'application/javascript; charset=utf-8');
  console.log('Matrix asset caching & gzip passed');

  // 5. Authenticated regular user login & auto-join (Power Level 0)
  console.log('--- 5. Testing authenticated user /api/matrix/sso-login (member PL 0) ---');
  const resUserLogin = await fetch(`${BASE_URL}/api/matrix/sso-login`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${testSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ device_id: 'q5KT0JowzT' })
  });
  assert.equal(resUserLogin.status, 200);
  const dataUserLogin = await resUserLogin.json();
  assert.equal(dataUserLogin.ok, true);
  assert.equal(dataUserLogin.role, 'member');
  assert.equal(dataUserLogin.powerLevel, 0);
  assert.equal(dataUserLogin.officialRoom, '#general:mitch.pro');
  assert.equal(dataUserLogin.device_id, 'q5KT0JowzT', 'SSO refresh must reuse the requested Matrix device');
  const reusedDeviceLogin = matrixLoginBodies.find(body => body.identifier?.user === 'matrixtestuser' && body.device_id === 'q5KT0JowzT');
  assert(reusedDeviceLogin, 'Server must forward the existing device_id to the Matrix homeserver');
  console.log('Regular member auto-provisioning passed:', dataUserLogin.user_id);

  // 6. Admin user auto-promotion (Power Level 100)
  console.log('--- 6. Testing Admin auto-promotion to Power Level 100 ---');
  const resAdminLogin = await fetch(`${BASE_URL}/api/matrix/sso-login`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}` }
  });
  assert.equal(resAdminLogin.status, 200);
  const dataAdminLogin = await resAdminLogin.json();
  assert.equal(dataAdminLogin.ok, true);
  assert.equal(dataAdminLogin.role, 'admin');
  assert.equal(dataAdminLogin.powerLevel, 100);
  assert.equal(mockPowerLevels.users['@admin:mitch.pro'], 100, 'Admin must be promoted to Power Level 100 in room');
  console.log('Admin auto-promoted successfully: @admin:mitch.pro -> PL 100');

  // 7. Moderator user auto-promotion (Power Level 50)
  console.log('--- 7. Testing Moderator auto-promotion to Power Level 50 ---');
  const resModLogin = await fetch(`${BASE_URL}/api/matrix/sso-login`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${modSid}` }
  });
  assert.equal(resModLogin.status, 200);
  const dataModLogin = await resModLogin.json();
  assert.equal(dataModLogin.ok, true);
  assert.equal(dataModLogin.role, 'moderator');
  assert.equal(dataModLogin.powerLevel, 50);
  assert.equal(mockPowerLevels.users['@matrixmoduser:mitch.pro'], 50, 'Moderator must be promoted to Power Level 50 in room');
  console.log('Moderator auto-promoted successfully: @matrixmoduser:mitch.pro -> PL 50');

  // 8. Intercept message report into Mitch.pro Chat Safety Reports
  console.log('--- 8. Testing Matrix report interception into CHAT_REPORTS_FILE ---');
  const reportEventId = '$bad_message_test_123';
  const resReport = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/report/${encodeURIComponent(reportEventId)}`, {
    method: 'POST',
    headers: {
      'Cookie': `studentId=${testSid}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({ reason: 'Toxic language in general chat', score: -100 })
  });
  assert.equal(resReport.status, 200);
  const savedReports = readDocument(CHAT_REPORTS_FILE, []);
  const foundReport = savedReports.find(r => r.matrixEventId === reportEventId || (r.id && r.id.includes('bad_message_test_123')));
  assert(foundReport, 'Report must be saved in chat_reports.json');
  assert(foundReport.reason.includes('Toxic language'));
  assert.equal(foundReport.matrixRoomId, '!official_general:mitch.pro');
  assert.equal(foundReport.context[0].from, '@test_spammer:mitch.pro');
  assert(foundReport.context[0].text.includes('test inappropriate message'));
  console.log('Matrix chat report intercepted and saved to safety dashboard:', foundReport.id);

  // 9. Matrix Moderation Overview API
  console.log('--- 9. Testing GET /api/matrix/moderation/overview ---');
  const resOverview = await fetch(`${BASE_URL}/api/matrix/moderation/overview`, {
    headers: { 'Cookie': `studentId=${adminSid}` }
  });
  assert.equal(resOverview.status, 200);
  const dataOverview = await resOverview.json();
  assert.equal(dataOverview.ok, true);
  assert.equal(dataOverview.officialRoom, '#general:mitch.pro');
  assert(Array.isArray(dataOverview.staff));
  const hasAdminStaff = dataOverview.staff.some(s => s.userId === '@admin:mitch.pro' && s.powerLevel === 100);
  const hasModStaff = dataOverview.staff.some(s => s.userId === '@matrixmoduser:mitch.pro' && s.powerLevel === 50);
  assert(hasAdminStaff, 'Admin must appear in staff roster');
  assert(hasModStaff, 'Moderator must appear in staff roster');
  console.log('Moderation overview passed with staff roster:', dataOverview.staff);

  // 10. Matrix Moderation Role Management (POST /api/matrix/moderation/set-role)
  console.log('--- 10. Testing POST /api/matrix/moderation/set-role ---');
  const resSetRole = await fetch(`${BASE_URL}/api/matrix/moderation/set-role`, {
    method: 'POST',
    headers: {
      'Cookie': `studentId=${adminSid}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({ userId: '@matrixtestuser:mitch.pro', powerLevel: 50 })
  });
  assert.equal(resSetRole.status, 200);
  assert.equal(mockPowerLevels.users['@matrixtestuser:mitch.pro'], 50);
  console.log('Role set successfully: @matrixtestuser:mitch.pro -> PL 50');

  // 11. Matrix Moderation Actions: Kick, Ban, Redact
  console.log('--- 11. Testing POST /api/matrix/moderation/kick, ban, redact ---');
  const resKick = await fetch(`${BASE_URL}/api/matrix/moderation/kick`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ userId: '@test_spammer:mitch.pro', reason: 'Trolling' })
  });
  assert.equal(resKick.status, 200);
  assert(kickedUsers.includes('@test_spammer:mitch.pro'), 'User must be kicked');

  const resBan = await fetch(`${BASE_URL}/api/matrix/moderation/ban`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ userId: '@test_spammer:mitch.pro', reason: 'Repeated offenses' })
  });
  assert.equal(resBan.status, 200);
  assert(bannedUsers.includes('@test_spammer:mitch.pro'), 'User must be banned');

  const resRedact = await fetch(`${BASE_URL}/api/matrix/moderation/redact`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ eventId: '$bad_message_test_123', reason: 'Violation' })
  });
  assert.equal(resRedact.status, 200);
  assert(redactedEvents.some(r => r.includes('$bad_message_test_123')), 'Message must be redacted');
  console.log('Kick, ban, redact actions executed successfully');

  // 12. Matrix Notifications fallback endpoint
  console.log('--- 12. Testing GET /_matrix/client/v3/notifications ---');
  const resNotifs = await fetch(`${BASE_URL}/_matrix/client/v3/notifications?limit=24`);
  assert.equal(resNotifs.status, 200, 'Notifications endpoint must return 200 OK');
  assert.equal(resNotifs.headers.get('Access-Control-Allow-Origin'), '*');
  const dataNotifs = await resNotifs.json();
  assert.deepEqual(dataNotifs, { notifications: [] }, 'Notifications endpoint must return empty notifications list');
  console.log('Notifications fallback passed');

  // 13. Matrix User-Interactive Authentication (UIA) password translation
  console.log('--- 13. Testing Matrix UIA with mitch.pro account password ---');
  // First test with valid mitch.pro password: must be translated to conduit password and succeed
  const resUiaValid = await fetch(`${BASE_URL}/_matrix/client/v3/keys/device_signing/upload`, {
    method: 'POST',
    headers: {
      'Authorization': 'Bearer tok_matrixtestuser',
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({
      auth: {
        type: 'm.login.password',
        identifier: { type: 'm.id.user', user: 'matrixtestuser' },
        password: 'mitch_test_pass_123'
      }
    })
  });
  assert.equal(resUiaValid.status, 200, 'UIA with valid mitch.pro password must succeed');
  const dataUiaValid = await resUiaValid.json();
  assert.equal(dataUiaValid.uia_authenticated, true);
  console.log('UIA valid password translation passed');

  // Second test with invalid password: must be rejected with 403
  const resUiaInvalid = await fetch(`${BASE_URL}/_matrix/client/v3/keys/device_signing/upload`, {
    method: 'POST',
    headers: {
      'Authorization': 'Bearer tok_matrixtestuser',
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({
      auth: {
        type: 'm.login.password',
        identifier: { type: 'm.id.user', user: 'matrixtestuser' },
        password: 'wrong_mitch_password'
      }
    })
  });
  assert.equal(resUiaInvalid.status, 403, 'UIA with invalid password must return 403');
  console.log('UIA invalid password rejection passed');

  // 14. Matrix Direct Login password translation
  console.log('--- 14. Testing Matrix direct login with mitch.pro account password ---');
  const resLoginValid = await fetch(`${BASE_URL}/_matrix/client/v3/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      type: 'm.login.password',
      identifier: { type: 'm.id.user', user: 'matrixtestuser' },
      password: 'mitch_test_pass_123'
    })
  });
  assert.equal(resLoginValid.status, 200, 'Direct login with valid mitch.pro password must succeed');
  const dataLoginValid = await resLoginValid.json();
  assert.equal(dataLoginValid.user_id, '@matrixtestuser:mitch.pro');
  assert(dataLoginValid.access_token);
  console.log('Direct login password translation passed');

  const resLoginInvalid = await fetch(`${BASE_URL}/_matrix/client/v3/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      type: 'm.login.password',
      identifier: { type: 'm.id.user', user: 'matrixtestuser' },
      password: 'wrong_mitch_password'
    })
  });
  assert.equal(resLoginInvalid.status, 403, 'Direct login with invalid password must return 403');
  console.log('Direct login invalid password rejection passed');

  // 15. Canonical Delivery Email: authentic dot preservation
  console.log('--- 15. Testing canonical email address resolution ---');
  // Check from names.json
  const resEmailStatus = await fetch(`${BASE_URL}/api/matrix/sso-status`, {
    headers: { 'Cookie': `studentId=${dottedSid}` }
  });
  assert.equal(resEmailStatus.status, 200);
  console.log('Canonical email resolution verified');

  // 16. Matrix Outbound Push Notification & Invite Dispatch
  console.log('--- 16. Testing Matrix outbound message and invite notifications ---');
  const mockSub = {
    endpoint: 'https://updates.push.services.mozilla.com/wpush/v2/test_sub_endpoint_1234567890',
    keys: {
      p256dh: 'BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4YfYCA_0QT9AcUbVYO-13e0U_M0tK1EtK2wtaz5Ry4YfYCA_0QT9AcUbVYO13e0U_M0t',
      auth: 'A1B2C3D4E5F6G7H8'
    }
  };
  const resSub = await fetch(`${BASE_URL}/api/push/subscribe`, {
    method: 'POST',
    headers: {
      'Cookie': `studentId=${adminSid}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(mockSub)
  });
  assert.equal(resSub.status, 200, 'Push subscribe should succeed');

  const subsOnDisk = readDocument(PUSH_SUBS_FILE, {});
  assert(subsOnDisk[adminNormEmail], 'Admin push subscription should be recorded in push_subs.json');

  const resSend = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/send/m.room.message/m_txn_1001`, {
    method: 'PUT',
    headers: {
      'Authorization': 'Bearer tok_matrixtestuser',
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({
      msgtype: 'm.text',
      body: 'Hello everyone in general chat!'
    })
  });
  assert.equal(resSend.status, 200, 'Sending Matrix message should return 200');
  const dataSend = await resSend.json();
  assert(dataSend.event_id, 'Send message response must contain event_id');

  for (const [version, eventType, content, expected] of [
    ['v3', 'm.room.message', { body: 'Try SCRAMJET' }, 403],
    ['r0', 'm.room.message', { body: 'hello', 'm.new_content': { body: 'ultraviolet' } }, 403],
    ['v3', 'm.room.message', { body: 'hello', formatted_body: '<b>pro</b>xy' }, 403],
    ['v3', 'm.room.message', { body: 'Torres explained the method' }, 200],
    ['v3', 'm.room.encrypted', { algorithm: 'm.megolm.v1.aes-sha2', ciphertext: 'opaque-test-content' }, 200],
  ]) {
    const filtered = await fetch(`${BASE_URL}/_matrix/client/${version}/rooms/!official_general:mitch.pro/send/${eventType}/policy_${randomBytes(4).toString('hex')}`, {
      method: 'PUT',
      headers: { Authorization: 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
      body: JSON.stringify(content),
    });
    assert.equal(filtered.status, expected, `Matrix word policy: ${version} ${eventType} ${JSON.stringify(content)}`);
    if (expected === 403) assert.equal((await filtered.json()).errcode, 'M_FORBIDDEN');
  }

  const resInvite = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/invite`, {
    method: 'POST',
    headers: {
      'Authorization': 'Bearer tok_matrixtestuser',
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({
      user_id: '@admin:mitch.pro'
    })
  });
  assert.equal(resInvite.status, 200, 'Room invite should return 200');

  // Verify Matrix notifications appear in the Mitch.pro notification bell API
  const resBell = await fetch(`${BASE_URL}/api/me/notifications`, {
    headers: { 'Cookie': `studentId=${adminSid}` }
  });
  assert.equal(resBell.status, 200, 'GET /api/me/notifications should return 200');
  const bellData = await resBell.json();
  assert(Array.isArray(bellData.notifications), 'Notifications response must contain notifications array');
  const matrixNotif = bellData.notifications.find(n => n.type === 'matrix' && n.matrixRoomId === '!official_general:mitch.pro');
  assert(matrixNotif, 'Matrix notification must appear in the bell list for recipient');
  assert(matrixNotif.url.includes('/matrix/#/room/'), 'Matrix notification must link to Matrix room');

  // Verify clearing Matrix notification via /api/matrix/notifications/read
  const resRead = await fetch(`${BASE_URL}/api/matrix/notifications/read`, {
    method: 'POST',
    headers: {
      'Cookie': `studentId=${adminSid}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({ roomId: '!official_general:mitch.pro' })
  });
  assert.equal(resRead.status, 200, 'POST /api/matrix/notifications/read should return 200');

  const resBellAfter = await fetch(`${BASE_URL}/api/me/notifications`, {
    headers: { 'Cookie': `studentId=${adminSid}` }
  });
  const bellDataAfter = await resBellAfter.json();
  const matrixNotifAfter = bellDataAfter.notifications.find(n => n.type === 'matrix' && n.matrixRoomId === '!official_general:mitch.pro');
  assert(!matrixNotifAfter, 'Cleared Matrix notification must no longer appear as unread');

  // Verify 24h email alert throttling: email sent timestamp must be recorded in matrix_email_sent.json
  const sentMap = readDocument(join(DATA_DIR, 'matrix_email_sent.json'), {});
  assert(sentMap[adminNormEmail], 'Admin user must have sent timestamp recorded in matrix_email_sent.json for 24h throttle');
  assert(Date.now() - sentMap[adminNormEmail] < 60_000, 'Sent timestamp must be recent');

  console.log('Matrix outbound message and invite notifications passed');

  // --- 17. Testing Matrix VoIP STUN/TURN Discovery ---
  console.log('--- 17. Testing Matrix VoIP STUN/TURN discovery ---');
  const resTurn = await fetch(`${BASE_URL}/_matrix/client/v3/voip/turnServer`, {
    headers: { 'Authorization': 'Bearer tok_matrixtestuser' }
  });
  assert.equal(resTurn.status, 200, 'GET /_matrix/client/v3/voip/turnServer should return 200');
  const turnData = await resTurn.json();
  assert(Array.isArray(turnData.uris) && turnData.uris.length > 0, 'turnServer must return ICE server URIs');
  assert(turnData.uris.some(u => u.startsWith('stun:')), 'turnServer must contain stun URIs');
  assert.equal(turnData.ttl, 86400, 'turnServer ttl must be 86400');
  console.log('Matrix VoIP STUN/TURN discovery passed');

  // --- 18. Testing Matrix client discovery with LiveKit RTC foci ---
  console.log('--- 18. Testing Matrix client discovery with LiveKit RTC foci ---');
  const resDiscovery = await fetch(`${BASE_URL}/.well-known/matrix/client`);
  assert.equal(resDiscovery.status, 200, 'GET /.well-known/matrix/client should return 200');
  const discoveryData = await resDiscovery.json();
  assert(discoveryData['m.homeserver'], 'client discovery must contain m.homeserver');
  const foci = discoveryData['org.matrix.msc4143.rtc_foci'];
  assert(Array.isArray(foci) && foci.length > 0, 'client discovery must contain org.matrix.msc4143.rtc_foci array');
  assert(foci.some(f => f.type === 'livekit' && typeof f.livekit_service_url === 'string'), 'rtc_foci must have livekit type and service url');
  console.log('Matrix client discovery with LiveKit RTC foci passed');

  // --- 19. Testing LiveKit SFU token generation ---
  console.log('--- 19. Testing LiveKit SFU token generation ---');
  const resSfuToken = await fetch(`${BASE_URL}/livekit/sfu/get`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      room: '!official_general:mitch.pro',
      user_id: '@matrixtestuser:mitch.pro'
    })
  });
  assert.equal(resSfuToken.status, 200, 'POST /livekit/sfu/get should return 200');
  const sfuData = await resSfuToken.json();
  assert(sfuData.url && sfuData.url.includes('/livekit/rtc'), 'LiveKit SFU response must contain rtc WebSocket URL');
  assert(sfuData.jwt, 'LiveKit SFU response must contain jwt token');
  const parts = sfuData.jwt.split('.');
  assert.equal(parts.length, 3, 'JWT must have 3 segments');
  const jwtPayload = JSON.parse(Buffer.from(parts[1], 'base64url').toString('utf8'));
  assert.equal(jwtPayload.sub, '@matrixtestuser:mitch.pro', 'JWT sub must match user_id');
  assert.equal(jwtPayload.video.room, '!official_general:mitch.pro', 'JWT room must match requested room');
  console.log('LiveKit SFU token generation passed');

  // --- 20. Testing Element Call runtime assets ---
  console.log('--- 20. Testing Element Call runtime assets ---');
  const resCallIndex = await fetch(`${BASE_URL}/matrix/public/element-call/index.html`);
  assert.equal(resCallIndex.status, 200, 'Element Call index.html must return 200');
  const callHtml = await resCallIndex.text();
  assert(callHtml.includes('Element Call') || callHtml.includes('Call'), 'Element Call HTML must load');
  const resCallConfig = await fetch(`${BASE_URL}/matrix/public/element-call/config.json`);
  assert.equal(resCallConfig.status, 200, 'Element Call config.json must return 200');
  console.log('Element Call runtime assets passed');

  // --- 21. Testing Matrix Slowmode configuration & enforcement ---
  console.log('--- 21. Testing Matrix Slowmode configuration & enforcement ---');
  // Non-admin cannot set slowmode
  const resSlowForbidden = await fetch(`${BASE_URL}/api/matrix/moderation/slowmode`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${testSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ seconds: 5 })
  });
  assert.equal(resSlowForbidden.status, 403, 'Non-admin setting slowmode must return 403');

  // Admin sets slowmode to 5 seconds
  const slowTestRoom = '!slowmode_test_room:mitch.pro';
  const resSlowSet = await fetch(`${BASE_URL}/api/matrix/moderation/slowmode`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ roomId: slowTestRoom, seconds: 5 })
  });
  assert.equal(resSlowSet.status, 200, 'Admin setting slowmode must return 200');
  const slowSetData = await resSlowSet.json();
  assert.equal(slowSetData.slowmodeSeconds, 5, 'Slowmode seconds must match 5');

  // Member sends first message: succeeds
  const resMsg1 = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/${encodeURIComponent(slowTestRoom)}/send/m.room.message/txn_slow_1`, {
    method: 'PUT',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Slowmode message 1' })
  });
  assert.equal(resMsg1.status, 200, 'First message from member should succeed');

  // Member sends second message immediately: must be rejected with 429 M_LIMIT_EXCEEDED
  const resMsg2 = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/${encodeURIComponent(slowTestRoom)}/send/m.room.message/txn_slow_2`, {
    method: 'PUT',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Slowmode message 2' })
  });
  assert.equal(resMsg2.status, 429, 'Immediate second message must be rejected with 429');
  const slowErrData = await resMsg2.json();
  assert.equal(slowErrData.errcode, 'M_LIMIT_EXCEEDED', 'Must return M_LIMIT_EXCEEDED');
  assert(slowErrData.retry_after_ms > 0, 'Must include retry_after_ms');
  assert(resMsg2.headers.get('Retry-After'), 'Must include Retry-After header');

  // Admin sending message is exempt from slowmode
  const resMsgAdmin = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/${encodeURIComponent(slowTestRoom)}/send/m.room.message/txn_admin_slow`, {
    method: 'PUT',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Authorization': 'Bearer tok_admin', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Admin message bypasses slowmode' })
  });
  assert.equal(resMsgAdmin.status, 200, 'Admin message must bypass slowmode');

  // Admin disables slowmode (0 seconds)
  const resSlowReset = await fetch(`${BASE_URL}/api/matrix/moderation/slowmode`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ roomId: slowTestRoom, seconds: 0 })
  });
  assert.equal(resSlowReset.status, 200);
  console.log('Matrix Slowmode configuration & 429 rate limiting passed');

  // --- 22. Testing Matrix User Muting & Unmuting ---
  console.log('--- 22. Testing Matrix User Muting & Unmuting ---');
  // Non-admin cannot mute
  const resMuteForbidden = await fetch(`${BASE_URL}/api/matrix/moderation/mute-user`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${testSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ userId: '@matrixtestuser:mitch.pro', durationSeconds: 60 })
  });
  assert.equal(resMuteForbidden.status, 403, 'Non-admin muting user must return 403');

  // Admin mutes user
  const resMute = await fetch(`${BASE_URL}/api/matrix/moderation/mute-user`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ userId: '@matrixtestuser:mitch.pro', durationSeconds: 60, reason: 'Spamming test' })
  });
  assert.equal(resMute.status, 200, 'Admin muting user must return 200');
  const muteData = await resMute.json();
  assert.equal(muteData.muted, true);

  // Muted user attempts to send message: rejected with 403 M_FORBIDDEN
  const resMutedSend = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/send/m.room.message/txn_muted`, {
    method: 'PUT',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Blocked muted message' })
  });
  assert.equal(resMutedSend.status, 403, 'Muted user send must return 403');
  const muteErrData = await resMutedSend.json();
  assert.equal(muteErrData.errcode, 'M_FORBIDDEN', 'Muted error code must be M_FORBIDDEN');
  assert(muteErrData.error.includes('muted'), 'Error message must state user is muted');

  // Overview includes muted user
  const resMuteOverview = await fetch(`${BASE_URL}/api/matrix/moderation/overview`, {
    headers: { 'Cookie': `studentId=${adminSid}` }
  });
  const overviewData = await resMuteOverview.json();
  assert(overviewData.mutedUsers.some(m => m.userId === '@matrixtestuser:mitch.pro'), 'Muted user must be in overview');

  // Admin unmutes user
  const resUnmute = await fetch(`${BASE_URL}/api/matrix/moderation/unmute-user`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ userId: '@matrixtestuser:mitch.pro' })
  });
  assert.equal(resUnmute.status, 200, 'Admin unmuting user must return 200');

  // User can send message again after unmute
  const resUnmutedSend = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/send/m.room.message/txn_unmuted`, {
    method: 'PUT',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Unmuted message works' })
  });
  assert.equal(resUnmutedSend.status, 200, 'Unmuted user sending message should succeed');
  console.log('Matrix User Muting & Unmuting passed');

  // --- 23. Testing Matrix Room Lockdown ---
  console.log('--- 23. Testing Matrix Room Lockdown ---');
  // Non-admin cannot lock down room
  const resLockForbidden = await fetch(`${BASE_URL}/api/matrix/moderation/mute-room`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${testSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ muted: true })
  });
  assert.equal(resLockForbidden.status, 403, 'Non-admin locking room must return 403');

  // Admin locks down room
  const resLock = await fetch(`${BASE_URL}/api/matrix/moderation/mute-room`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ muted: true })
  });
  assert.equal(resLock.status, 200);

  // Member send rejected during lockdown
  const resLockSend = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/send/m.room.message/txn_locked`, {
    method: 'PUT',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Should be blocked by lockdown' })
  });
  assert.equal(resLockSend.status, 403, 'Member message must be rejected in lockdown');

  // Admin send succeeds during lockdown
  const resLockAdminSend = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/send/m.room.message/txn_admin_locked`, {
    method: 'PUT',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Authorization': 'Bearer tok_admin', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Admin announcement during lockdown' })
  });
  assert.equal(resLockAdminSend.status, 200, 'Admin send must succeed during lockdown');

  // Admin unlocks room
  const resUnlock = await fetch(`${BASE_URL}/api/matrix/moderation/mute-room`, {
    method: 'POST',
    headers: { 'Cookie': `studentId=${adminSid}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ muted: false })
  });
  assert.equal(resUnlock.status, 200);

  // Member send succeeds again
  const resUnlockedSend = await fetch(`${BASE_URL}/_matrix/client/v3/rooms/!official_general:mitch.pro/send/m.room.message/txn_unlocked`, {
    method: 'PUT',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ msgtype: 'm.text', body: 'Unlocked room message' })
  });
  assert.equal(resUnlockedSend.status, 200, 'Member send must succeed after unlocking room');
  console.log('Matrix Room Lockdown passed');

  // --- 24. Testing Matrix Stale Device Pruning ---
  console.log('--- 24. Testing Matrix Stale Device Pruning ---');
  const resPrune = await fetch(`${BASE_URL}/api/matrix/devices/prune-stale`, {
    method: 'POST',
    headers: { 'Authorization': 'Bearer tok_matrixtestuser', 'Content-Type': 'application/json' },
    body: JSON.stringify({ currentDeviceId: 'DEV_CURRENT', maxAgeDays: 7 })
  });
  assert.equal(resPrune.status, 200, 'Pruning stale devices must return 200');
  const pruneData = await resPrune.json();
  assert.equal(pruneData.ok, true);
  assert.equal(pruneData.prunedCount, 2, 'Should prune 2 stale devices');
  assert.equal(pruneData.remainingCount, 1, 'Should retain 1 active current device');
  assert(deletedDeviceIds.includes('DEV_OLD_1') && deletedDeviceIds.includes('DEV_OLD_2'), 'Both old devices must be deleted');
  console.log('Matrix Stale Device Pruning passed');

  console.log('=== ALL MATRIX SSO & MODERATION UNIT TESTS PASSED SUCCESSFULLY! ===');
} finally {
  writeDocument(MATRIX_NOTIFS_FILE, origNotifs);
  writeDocument(MATRIX_EMAIL_SENT_FILE, origEmailSent);
  writeDocument(MATRIX_ROOM_SETTINGS_FILE, origRoomSettings);
  writeDocument(MODERATORS_FILE, origMods);
  writeDocument(CHAT_REPORTS_FILE, origReports);
  writeDocument(PASSWORDS_FILE, origPasswords);
  writeDocument(PUSH_SUBS_FILE, origPushSubs);
  mockConduit.stop();
  serverProc.kill();
}
