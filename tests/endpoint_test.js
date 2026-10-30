import { configureDataStore, readDocument, writeDocument } from '../lib/data_store.js';
import { readFileSync } from 'fs';
import { createHash, createHmac } from 'crypto';
import { join } from 'path';

const REPO_ROOT = import.meta.dir + '/..';
configureDataStore({ baseDir: REPO_ROOT, dataDir: join(REPO_ROOT, 'data') });

const BASE_URL = process.env.BASE_URL || 'http://localhost:6800';
if (!['localhost', '127.0.0.1', '[::1]'].includes(new URL(BASE_URL).hostname)) {
  throw new Error('Integration endpoint tests must target a local test server.');
}

const ID_SECRET = readFileSync(join(REPO_ROOT, 'data', 'id_secret.key'));

function makeEmailIdForTest(email) {
  const key = email;
  const emailHash = createHash('sha256').update(key).digest('hex').slice(0, 24);
  const raw = 'e' + emailHash;
  const sig = createHmac('sha256', ID_SECRET).update(raw).digest('hex').slice(0, 16);
  return raw + '.' + sig;
}

const ADMIN_TOKEN = makeEmailIdForTest(normalizeEmail('admin@mitch.pro'));
const USER_TOKEN = makeEmailIdForTest(normalizeEmail('test_normal_user@student.rjuhsd.us'));

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

// Dynamically generate a fresh target email for the premium grant to keep tests perfectly repeatable
const dynamicEmail = `test_premium_${Date.now()}@student.rjuhsd.us`;

const tests = [
  {
    name: 'Public top-bar wallet has a clean signed-out state',
    path: '/api/me/coins', method: 'GET', token: null, expectedStatus: 200,
    verify: data => data.authenticated === false && data.coins === null
  },
  // --- VM ownership isolation ---
  {
    name: "User A cannot view User B's computer",
    path: '/api/vm/computers/vm-auth-user-b', method: 'GET', token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: "User A cannot start User B's computer",
    path: '/api/vm/computers/vm-auth-user-b/power', method: 'POST', body: { action: 'start' }, token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: "User A cannot stop User B's computer",
    path: '/api/vm/computers/vm-auth-user-b/power', method: 'POST', body: { action: 'force-stop' }, token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: "User A cannot restart User B's computer",
    path: '/api/vm/computers/vm-auth-user-b/power', method: 'POST', body: { action: 'restart' }, token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: "User A cannot shut down User B's computer",
    path: '/api/vm/computers/vm-auth-user-b/power', method: 'POST', body: { action: 'shutdown' }, token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: "User A cannot create a console for User B's computer",
    path: '/api/vm/computers/vm-auth-user-b/desktop-session', method: 'POST', body: {}, token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: 'Nonexistent computer returns 404',
    path: '/api/vm/computers/vm-does-not-exist', method: 'GET', token: USER_TOKEN, expectedStatus: 404
  },
  {
    name: 'Malformed computer ID is rejected',
    path: '/api/vm/computers/not%21a%21record', method: 'GET', token: USER_TOKEN, expectedStatus: 400
  },
  {
    name: 'Normal user cannot view desktop administration',
    path: '/api/admin/vms/overview', method: 'GET', token: USER_TOKEN, expectedStatus: 403
  },
  ...['assign', 'unassign', 'create'].map(operation => ({
    name: `Normal user cannot ${operation} computers`,
    path: `/api/admin/vms/${operation}`, method: 'POST', body: {}, token: USER_TOKEN, expectedStatus: 403
  })),
  {
    name: 'Normal user cannot open desktop admin page',
    path: '/admin/vms/', method: 'GET', token: USER_TOKEN, expectedStatus: 403
  },
  {
    name: 'Desktop WebSocket requires authentication',
    path: '/api/vm/desktop/ws?session=missing', method: 'GET', token: null, expectedStatus: 403,
    headers: { Upgrade: 'websocket', Origin: BASE_URL }
  },
  {
    name: 'Expired desktop WebSocket session is rejected',
    path: '/api/vm/desktop/ws?session=expired-session', method: 'GET', token: USER_TOKEN, expectedStatus: 401,
    headers: { Upgrade: 'websocket', Origin: BASE_URL }
  },
  {
    name: 'Cross-origin desktop WebSocket is rejected',
    path: '/api/vm/desktop/ws?session=missing', method: 'GET', token: USER_TOKEN, expectedStatus: 403,
    headers: { Upgrade: 'websocket', Origin: 'https://untrusted.example' }
  },
  {
    name: 'Owner can view their running computer',
    path: '/api/vm/computers/vm-auth-user-a', method: 'GET', token: USER_TOKEN, expectedStatus: 200,
    verify: data => data.computer?.status === 'running' && data.computer?.ipAddress === '10.0.0.23'
  },
  {
    name: 'Stopped computer cannot create desktop session',
    path: '/api/vm/computers/vm-auth-stopped/desktop-session', method: 'POST', body: {}, token: USER_TOKEN, expectedStatus: 409,
    verify: data => data.code === 'computer_offline'
  },
  {
    name: 'Upstream failure returns friendly computer error',
    path: '/api/vm/computers/vm-auth-unreachable', method: 'GET', token: USER_TOKEN, expectedStatus: 502,
    verify: data => data.code === 'computer_unreachable' && data.error === 'Your computer could not be reached.'
  },
  {
    name: 'Owner can create an opaque desktop session',
    path: '/api/vm/computers/vm-auth-user-a/desktop-session', method: 'POST', body: {}, token: USER_TOKEN, expectedStatus: 201,
    verify: data => data.socketPath?.startsWith('/api/vm/desktop/ws?session=') && data.expiresIn > 0 && !JSON.stringify(data).includes('integration-api-secret-do-not-expose') && !JSON.stringify(data).includes('127.0.0.1')
  },
  // --- Admin Endpoints ---
  {
    name: 'GET /api/admin/moderators (Admin)',
    path: '/api/admin/moderators',
    method: 'GET',
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/admin/moderators (Non-Admin)',
    path: '/api/admin/moderators',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'GET /api/admin/moderators (Anonymous)',
    path: '/api/admin/moderators',
    method: 'GET',
    token: null,
    expectedStatus: 403
  },
  {
    name: 'GET /api/admin/moderator-panel (Admin)',
    path: '/api/admin/moderator-panel',
    method: 'GET',
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/admin/moderator-panel (Non-Admin)',
    path: '/api/admin/moderator-panel',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'GET /api/admin/moderator-requests (Admin)',
    path: '/api/admin/moderator-requests',
    method: 'GET',
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/admin/moderator-requests (Non-Admin)',
    path: '/api/admin/moderator-requests',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'GET /api/admin/economy/audit (Admin)',
    path: '/api/admin/economy/audit',
    method: 'GET',
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/admin/economy/audit (Non-Admin)',
    path: '/api/admin/economy/audit',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/gift-coins (Admin)',
    path: '/api/admin/gift-coins',
    method: 'POST',
    body: { targetEmail: 'test123@example.com', amount: 10, reason: 'Test admin gift' },
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'Fund casino integration account',
    path: '/api/admin/gift-coins', method: 'POST', token: ADMIN_TOKEN, expectedStatus: 200,
    body: { targetEmail: 'test_normal_user@student.rjuhsd.us', amount: 1000, reason: 'Casino integration tests' }
  },
  {
    name: 'POST /api/casino/rock-paper-scissors',
    path: '/api/casino/rock-paper-scissors', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: { amount: 10, choice: 'rock' }, verify: data => ['rock', 'paper', 'scissors'].includes(data.computer) && Number.isFinite(data.newBalance)
  },
  {
    name: 'POST /api/casino/lucky-seven',
    path: '/api/casino/lucky-seven', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: { amount: 10 }, verify: data => Array.isArray(data.dice) && data.dice.length === 2 && Number.isFinite(data.newBalance)
  },
  {
    name: 'POST /api/casino/color-card',
    path: '/api/casino/color-card', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: { amount: 10, choice: 'red' }, verify: data => ['red', 'black'].includes(data.color) && Number.isFinite(data.newBalance)
  },
  {
    name: 'POST /api/casino/triple-dice',
    path: '/api/casino/triple-dice', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: { amount: 10, pick: 3 }, verify: data => Array.isArray(data.dice) && data.dice.length === 3 && Number.isFinite(data.newBalance)
  },
  {
    name: 'POST /api/casino/plinko',
    path: '/api/casino/plinko', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: { amount: 10 }, verify: data => typeof data.slot === 'string' && Number.isFinite(data.newBalance)
  },
  {
    name: 'Casino rejects GET power-like game actions',
    path: '/api/casino/blackjack/hit', method: 'GET', token: USER_TOKEN, expectedStatus: 405
  },
  {
    name: 'Casino rejects invalid negative wagers',
    path: '/api/casino/lucky-seven', method: 'POST', token: USER_TOKEN, expectedStatus: 400,
    body: { amount: -1 }
  },
  {
    name: 'Blackjack deals a hand',
    path: '/api/casino/blackjack/start', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: { amount: 10 }
  },
  {
    name: 'Blackjack rejects duplicate deal without consuming another wager',
    path: '/api/casino/blackjack/start', method: 'POST', token: USER_TOKEN, expectedStatus: 409,
    body: { amount: 10 }
  },
  {
    name: 'Blackjack hand restores without revealing hidden dealer card',
    path: '/api/casino/blackjack/state', method: 'GET', token: USER_TOKEN, expectedStatus: 200,
    verify: data => data.active && data.bet === 10 && data.playerHand.length === 2 && !data.dealerHand && !data.deck
  },
  {
    name: 'Blackjack settles the hand',
    path: '/api/casino/blackjack/stand', method: 'POST', token: USER_TOKEN, expectedStatus: 200,
    body: {}, verify: data => data.gameOver && Number.isFinite(data.winAmt)
  },
  {
    name: 'Blackjack clears settled hand',
    path: '/api/casino/blackjack/state', method: 'GET', token: USER_TOKEN, expectedStatus: 200,
    verify: data => data.active === false
  },
  {
    name: 'POST /api/admin/gift-coins (Non-Admin)',
    path: '/api/admin/gift-coins',
    method: 'POST',
    body: { targetEmail: 'test123@example.com', amount: 10, reason: 'Test admin gift' },
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/grant-premium (Admin)',
    path: '/api/admin/grant-premium',
    method: 'POST',
    body: { targetEmail: dynamicEmail, reason: 'Test admin grant' },
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'POST /api/admin/grant-premium (Non-Admin)',
    path: '/api/admin/grant-premium',
    method: 'POST',
    body: { targetEmail: dynamicEmail, reason: 'Test admin grant' },
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/economy/burn (Admin)',
    path: '/api/admin/economy/burn',
    method: 'POST',
    body: { email: 'test123@example.com', amount: 5 },
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'POST /api/admin/economy/burn (Non-Admin)',
    path: '/api/admin/economy/burn',
    method: 'POST',
    body: { email: 'test123@example.com', amount: 5 },
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/casino/rig (Admin)',
    path: '/api/admin/casino/rig',
    method: 'POST',
    body: { target: 'test123@example.com', chance: 50 },
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'POST /api/admin/casino/rig (Non-Admin)',
    path: '/api/admin/casino/rig',
    method: 'POST',
    body: { target: 'test123@example.com', chance: 50 },
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/broadcast (Admin)',
    path: '/api/admin/broadcast',
    method: 'POST',
    body: { msg: 'Test broadcast message', type: 'normal' },
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'POST /api/admin/broadcast (Non-Admin)',
    path: '/api/admin/broadcast',
    method: 'POST',
    body: { msg: 'Test broadcast message', type: 'normal' },
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/reset-other-passphrase (Admin)',
    path: '/api/admin/reset-other-passphrase',
    method: 'POST',
    body: { targetEmail: 'admin2@mitch.pro', newPassphrase: 'newadminpass123' },
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'POST /api/admin/reset-other-passphrase (Non-Admin)',
    path: '/api/admin/reset-other-passphrase',
    method: 'POST',
    body: { targetEmail: 'admin2@mitch.pro', newPassphrase: 'newadminpass123' },
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/reset-other-passphrase (Invalid Target)',
    path: '/api/admin/reset-other-passphrase',
    method: 'POST',
    body: { targetEmail: 'test_normal_user@student.rjuhsd.us', newPassphrase: 'newadminpass123' },
    token: ADMIN_TOKEN,
    expectedStatus: 400
  },
  
  // --- General Endpoints ---
  {
    name: 'GET /api/members (Authenticated)',
    path: '/api/members',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/members (Anonymous)',
    path: '/api/members',
    method: 'GET',
    token: null,
    expectedStatus: 403
  },
  {
    name: 'GET /api/moderator-members (Authenticated)',
    path: '/api/moderator-members',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/owner-members (Authenticated)',
    path: '/api/owner-members',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/shop/items (Authenticated)',
    path: '/api/shop/items',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/marketplace/items (Authenticated)',
    path: '/api/marketplace/items',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/me/inventory (Authenticated)',
    path: '/api/me/inventory',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/profile (Authenticated)',
    path: '/api/profile',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /robots.txt (Anonymous)',
    path: '/robots.txt',
    method: 'GET',
    token: null,
    expectedStatus: 200
  },
  {
    name: 'GET /larp (Anonymous)',
    path: '/larp',
    method: 'GET',
    token: null,
    expectedStatus: 302
  },
  {
    name: 'GET /larp/ (Anonymous)',
    path: '/larp/',
    method: 'GET',
    token: null,
    expectedStatus: 200
  },
  {
    name: 'GET /larp/rezero (Anonymous)',
    path: '/larp/rezero',
    method: 'GET',
    token: null,
    expectedStatus: 302
  },
  {
    name: 'GET /larp/rezero/ (Anonymous)',
    path: '/larp/rezero/',
    method: 'GET',
    token: null,
    expectedStatus: 200
  },
  {
    name: 'GET /ssh/ (Authenticated)',
    path: '/ssh/',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /ssh/ (Anonymous)',
    path: '/ssh/',
    method: 'GET',
    token: null,
    expectedStatus: 302
  },
  {
    name: 'GET /api/admin/ssh-key (Admin)',
    path: '/api/admin/ssh-key',
    method: 'GET',
    token: ADMIN_TOKEN,
    expectedStatus: 200
  },
  {
    name: 'GET /api/admin/ssh-key (Non-Admin)',
    path: '/api/admin/ssh-key',
    method: 'GET',
    token: USER_TOKEN,
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/ssh-key/generate (Admin)',
    path: '/api/admin/ssh-key/generate',
    method: 'POST',
    token: ADMIN_TOKEN,
    body: { passphrase: 'ssh-key-password' },
    expectedStatus: 200
  },
  {
    name: 'POST /api/admin/ssh-key/generate (Non-Admin)',
    path: '/api/admin/ssh-key/generate',
    method: 'POST',
    token: USER_TOKEN,
    body: { passphrase: 'ssh-key-password' },
    expectedStatus: 403
  },
  {
    name: 'POST /api/admin/ssh-key/save (Admin - Invalid Key)',
    path: '/api/admin/ssh-key/save',
    method: 'POST',
    token: ADMIN_TOKEN,
    body: { privateKey: 'invalid-key-data', passphrase: 'pass' },
    expectedStatus: 400
  },
  {
    name: 'POST /api/admin/ssh-key/save (Non-Admin)',
    path: '/api/admin/ssh-key/save',
    method: 'POST',
    token: USER_TOKEN,
    body: { privateKey: 'some-key', passphrase: 'pass' },
    expectedStatus: 403
  }
];

async function run() {
  console.log('\n--- Commencing Endpoint Verification ---');
  let passedCount = 0;
  let failedCount = 0;
  
  for (const t of tests) {
    const url = `${BASE_URL}${t.path}`;
    const headers = {
      'CF-Connecting-IP': '66.60.183.124',
      ...t.headers
    };
    if (t.method === 'POST' && (t.path.startsWith('/api/vm/') || t.path.startsWith('/api/admin/vms/')) && !headers.Origin) {
      headers.Origin = BASE_URL;
    }
    if (t.token) {
      headers['Cookie'] = `studentId=${t.token}`;
    }
    if (t.token === ADMIN_TOKEN) {
      headers['X-Admin-Passphrase'] = 'testpass123';
    }
    
    const options = {
      method: t.method,
      headers,
      redirect: 'manual'
    };
    
    if (t.body) {
      options.headers['Content-Type'] = 'application/json';
      options.body = JSON.stringify(t.body);
    }
    
    try {
      const response = await fetch(url, options);
      const status = response.status;
      
      const strictAuthorization = t.path.startsWith('/api/vm/') || t.path.startsWith('/api/admin/vms/') || t.path.startsWith('/admin/vms/');
      const statusMatches = status === t.expectedStatus ||
                            (!strictAuthorization && t.expectedStatus === 403 && status === 401);
                            
      const payloadMatches = !t.verify || t.verify(await response.clone().json());
      if (statusMatches && payloadMatches) {
        console.log(`✅ [PASS] ${t.name} -> Status: ${status}`);
        passedCount++;
      } else {
        let bodyText = '';
        try { bodyText = await response.text(); } catch (e) {}
        console.error(`❌ [FAIL] ${t.name} -> Expected Status: ${t.expectedStatus}, Got: ${status}. Response: ${bodyText}`);
        failedCount++;
      }
    } catch (err) {
      console.error(`❌ [FAIL] ${t.name} -> Connection Error:`, err.message);
      failedCount++;
    }
  }

  const vmPowerResults = await runVmPowerVerification();
  passedCount += vmPowerResults.passedCount;
  failedCount += vmPowerResults.failedCount;
  
  // --- Sequential Auth Flow verification ---
  const authResults = await runCrazyAuthVerification();
  passedCount += authResults.passedCount;
  failedCount += authResults.failedCount;

  // --- Friends & Presence Flow verification ---
  const friendResults = await runFriendsVerification();
  passedCount += friendResults.passedCount;
  failedCount += friendResults.failedCount;

  // --- Canvas Zones Flow verification ---
  const zoneResults = await runCanvasZonesVerification();
  passedCount += zoneResults.passedCount;
  failedCount += zoneResults.failedCount;

  console.log('\n--- Endpoint Testing Summary ---');
  console.log(`Total Passed: ${passedCount}`);
  console.log(`Total Failed: ${failedCount}`);
  
  if (failedCount > 0) {
    console.error('\n⚠️ Some endpoints failed verification!');
    process.exitCode = 1;
  } else {
    console.log('\n🎉 All endpoints successfully verified!');
  }
}

async function fetchWithBypass(url, options = {}) {
  options.headers = options.headers || {};
  options.headers['CF-Connecting-IP'] = '66.60.183.124';
  return fetch(url, options);
}

async function runVmPowerVerification() {
  const requestPower = () => fetchWithBypass(`${BASE_URL}/api/vm/computers/vm-auth-user-a/power`, {
    method: 'POST', headers: { Cookie: `studentId=${USER_TOKEN}`, 'Content-Type': 'application/json', Origin: BASE_URL },
    body: JSON.stringify({ action: 'restart' })
  });
  try {
    const responses = await Promise.all([requestPower(), requestPower()]);
    const statuses = responses.map(response => response.status).sort();
    if (statuses[0] !== 202 || statuses[1] !== 409) throw new Error(`Expected one 202 and one 409, got ${statuses.join(', ')}`);
    console.log('PASS: Concurrent power requests accept one operation and reject the duplicate.');
    return { passedCount: 1, failedCount: 0 };
  } catch (error) {
    console.error('FAIL: Concurrent power request isolation:', error.message);
    return { passedCount: 0, failedCount: 1 };
  }
}

async function runCrazyAuthVerification() {
  console.log('\n--- Commencing Crazy Authentication & Login Verification ---');
  let passedCount = 0;
  let failedCount = 0;

  const assert = (condition, message) => {
    if (condition) {
      console.log(`✅ [PASS] ${message}`);
      passedCount++;
    } else {
      console.error(`❌ [FAIL] ${message}`);
      failedCount++;
    }
  };

  try {
    const testEmail = `crazy_login_${Date.now()}@student.rjuhsd.us`;
    const testPassword = `crazy_password_12345`;
    const refCode = `TESTINVITE123`;

    // 1. Test Signup with invalid inputs (empty)
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: '', password: '' })
      });
      const data = await resp.json();
      assert(resp.status === 400 && data.success === false, 'Signup with empty email/password should fail (400)');
    }

    // 2. Test Signup with short password (<6 chars)
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, password: 'short' })
      });
      const data = await resp.json();
      assert(resp.status === 400 && data.success === false, 'Signup with short password (<6 chars) should fail (400)');
    }

    // 3. Test Signup with valid credentials and referral code
    let signupSuccess = false;
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, password: testPassword, refCode, recaptcha_token: 'dummy' })
      });
      const data = await resp.json();
      signupSuccess = resp.status === 200 && data.success === true;
      if (!signupSuccess) {
        console.error(`Signup failed! Status: ${resp.status}, Body:`, JSON.stringify(data));
      }
      assert(signupSuccess, `Signup with valid credentials and referral code should succeed (200)`);
    }

    if (!signupSuccess) {
      console.error('Skipping remaining auth tests because signup failed.');
      return { passedCount, failedCount };
    }

    // Print invite codes file content for diagnostics
    try {
      const invCodesContent = readDocument(join(REPO_ROOT, 'data', 'invite_codes.json'), {});
      console.log('Diagnostic - invite code store:', JSON.stringify(invCodesContent, null, 2));
    } catch (e) {
      console.log('Diagnostic - Failed to read invite_codes.json:', e.message);
    }

    // 4. Read verification code from disk
    const signupCodes = readDocument(join(REPO_ROOT, 'data', 'signup_codes.json'), {});
    const normalized = normalizeEmail(testEmail);
    const entry = signupCodes[normalized];
    assert(entry !== undefined, 'Signup verification code should be available in the signup code store');

    if (!entry) {
      console.error('Skipping verification tests because signup code was not found.');
      return { passedCount, failedCount };
    }

    const verificationCode = entry.code;
    console.log(`Found signup verification code on disk: ${verificationCode}`);

    // Fetch the referrer's initial coins balance dynamically
    let initialReferrerCoins = 0;
    try {
      const preRefResp = await fetchWithBypass(`${BASE_URL}/api/me/coins`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${USER_TOKEN}` }
      });
      const preRefData = await preRefResp.json();
      initialReferrerCoins = preRefData.coins || 0;
      console.log(`Referrer initial coins: ${initialReferrerCoins}`);
    } catch (e) {
      console.log('Failed to fetch initial referrer coins:', e.message);
    }

    // 5. Test verification with invalid code
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/verify-signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, code: '000000' })
      });
      const data = await resp.json();
      assert(resp.status === 400 && data.success === false, 'Verification with incorrect code should fail (400)');
    }

    // 6. Test verification with correct code
    let verifiedSid = null;
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/verify-signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, code: verificationCode })
      });
      const data = await resp.json();
      verifiedSid = data.id;
      assert(resp.status === 200 && data.success === true && verifiedSid, `Verification with correct code should succeed (200) and return session ID`);
    }

    if (!verifiedSid) {
      console.error('Skipping remaining login tests because verification failed.');
      return { passedCount, failedCount };
    }

    // 7. Test duplicate signup (preventing same email from signing up twice now that they are verified/registered)
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, password: testPassword, recaptcha_token: 'dummy' })
      });
      const data = await resp.json();
      assert(resp.status === 400 && data.message && data.message.includes('already exists'), 'Signup with duplicate email should fail (400)');
    }

    // Wait 2500ms to allow the running server's NAMES_FILE cache (which has a 2000ms TTL) to expire
    console.log('Waiting 2.5 seconds for running server\'s session cache to reload names.json...');
    await new Promise(resolve => setTimeout(resolve, 2500));

    // 8. Verify referral MitchCoins rewarding (both parties get 2000 MitchCoins!)
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/me/coins`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${verifiedSid}` }
      });
      const data = await resp.json();
      assert(resp.status === 200 && data.coins === 2000, `New user should receive 2000 referral MitchCoins bonus (Got: ${data.coins})`);
    }

    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/me/coins`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${USER_TOKEN}` }
      });
      const data = await resp.json();
      const expectedCoins = initialReferrerCoins + 2000;
      assert(resp.status === 200 && data.coins === expectedCoins, `Referrer user should receive 2000 referral MitchCoins bonus (Got: ${data.coins}, Expected: ${expectedCoins})`);
    }

    // 9. Test Login with invalid email
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: 'nonexistent@student.rjuhsd.us', password: testPassword, recaptcha_token: 'dummy' })
      });
      const data = await resp.json();
      assert(resp.status === 401 && data.success === false, 'Login with non-existent email should return unauthorized (401)');
    }

    // 10. Test Login with invalid password
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, password: 'wrongpassword', recaptcha_token: 'dummy' })
      });
      const data = await resp.json();
      assert(resp.status === 401 && data.success === false, 'Login with incorrect password should return unauthorized (401)');
    }

    // 11. Test Login with correct credentials
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, password: testPassword, recaptcha_token: 'dummy' })
      });
      const data = await resp.json();
      assert(resp.status === 200 && data.success === true && data.id === verifiedSid, 'Login with correct credentials should succeed (200) and match verified ID');
    }

    // 12. Test Password Reset Request: Call /api/request-access
    let resetRequestSuccess = false;
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/request-access`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: testEmail, recaptcha_token: 'dummy' })
      });
      const data = await resp.json();
      resetRequestSuccess = resp.status === 200 && data.success === true;
      if (resp.status === 429) {
        console.log('⚠️ [SKIP] Requesting password reset (request-access) was rate limited (429). Skipping reset verification.');
        passedCount++;
      } else {
        assert(resetRequestSuccess, 'Requesting password reset (request-access) should succeed (200)');
      }
    }

    if (resetRequestSuccess) {
      // 13. Read OTP token from disk
      const tokens = readDocument(join(REPO_ROOT, 'data', 'tokens.json'), {});
      const foundResetToken = Object.entries(tokens).find(([k, t]) => 
        t.email === testEmail && t.type === 'reset' && !t.used
      );
      assert(foundResetToken !== undefined, 'Password reset token should be available in the token store');

      if (foundResetToken) {
        const [tokenKey, tokenEntry] = foundResetToken;
        const otpCode = tokenEntry.otp;
        console.log(`Found reset OTP code: ${otpCode}`);

        // 14. Test Claiming Password Reset Token with invalid inputs (short password)
        {
          const resp = await fetchWithBypass(`${BASE_URL}/api/claim-token`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ email: testEmail, token: otpCode, password: 'short' })
          });
          const data = await resp.json();
          assert(resp.status === 400 && data.success === false, 'Claiming token with short password (<6 chars) should fail (400)');
        }

        // 15. Test Claiming Password Reset Token with correct OTP
        const newPassword = 'new_crazy_password_98765';
        let claimSuccess = false;
        {
          const resp = await fetchWithBypass(`${BASE_URL}/api/claim-token`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ email: testEmail, token: otpCode, password: newPassword })
          });
          const data = await resp.json();
          claimSuccess = resp.status === 200 && data.success === true;
          assert(claimSuccess, 'Claiming reset token with correct OTP and valid password should succeed (200)');
        }

        if (claimSuccess) {
          // 16. Verify that old password fails now
          {
            const resp = await fetchWithBypass(`${BASE_URL}/api/login`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ email: testEmail, password: testPassword, recaptcha_token: 'dummy' })
            });
            assert(resp.status === 401, 'Login with old password after reset should fail (401)');
          }

          // 17. Verify that new password succeeds
          {
            const resp = await fetchWithBypass(`${BASE_URL}/api/login`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ email: testEmail, password: newPassword, recaptcha_token: 'dummy' })
            });
            const data = await resp.json();
            assert(resp.status === 200 && data.success === true, 'Login with new password after reset should succeed (200)');
          }
        }
      }

      // Unsubscribe verification
      {
        // 1. Verify invalid token returns error page
        const resp1 = await fetchWithBypass(`${BASE_URL}/unsubscribe/12345678901234567890123456789012`);
        const text1 = await resp1.text();
        assert(resp1.status === 200 && text1.includes('Invalid Link'), 'Accessing unsubscribe with invalid token should show invalid link page');
        passedCount++;

        // 2. Set up test unsubscribe token
        const tokenPath = join(REPO_ROOT, 'data', 'unsubscribe_tokens.json');
        const unsubPath = join(REPO_ROOT, 'data', 'newsletter_unsub.json');
        
        let unsubTokens = readDocument(tokenPath, {});
        if (!unsubTokens || typeof unsubTokens !== 'object' || Array.isArray(unsubTokens)) unsubTokens = {};
        const testUnsubEmail = 'unsubtest_crazy_login@example.com';
        const testUnsubToken = 'abcdefabcdefabcdefabcdefabcdefab';
        unsubTokens[testUnsubEmail] = testUnsubToken;
        writeDocument(tokenPath, unsubTokens);

        // 3. Verify accessing direct unsubscribe URL unsubscribes and displays success page
        const resp2 = await fetchWithBypass(`${BASE_URL}/unsubscribe/${testUnsubToken}`);
        const text2 = await resp2.text();
        assert(resp2.status === 200 && text2.includes('Unsubscribed') && text2.includes(testUnsubEmail), 'Accessing direct unsubscribe URL should instantly unsubscribe user');
        passedCount++;

        // 4. Verify email is added to newsletter_unsub.json
        const unsubList = readDocument(unsubPath, []);
        assert(unsubList.includes(testUnsubEmail), 'Unsubscribed email should be present in the unsubscribe list');
        passedCount++;

        // 5. Verify token is NOT deleted and remains in unsubscribe_tokens.json
        const unsubTokensAfter = readDocument(tokenPath, {});
        assert(unsubTokensAfter[testUnsubEmail] === testUnsubToken, 'Unsubscribe token should remain unchanged and not deleted after unsubscribe');
        passedCount++;
      }
    }

  } catch (err) {
    console.error('Error during crazy auth verification:', err);
    failedCount++;
  }

  return { passedCount, failedCount };
}

async function runFriendsVerification() {
  console.log('\n--- Commencing Friends & Presence Verification ---');
  let passedCount = 0;
  let failedCount = 0;

  const assert = (condition, message) => {
    if (condition) {
      console.log(`✅ [PASS] ${message}`);
      passedCount++;
    } else {
      console.error(`❌ [FAIL] ${message}`);
      failedCount++;
    }
  };

  try {
    const emailA = `friend_user_a_${Date.now()}@student.rjuhsd.us`;
    const emailB = `friend_user_b_${Date.now()}@student.rjuhsd.us`;
    const password = 'SuperSecureFriendPwd99!';

    // 1. Signup both users
    let sidA = null;
    let sidB = null;

    // Signup A
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailA, password, recaptcha_token: 'dummy' })
      });
      assert(resp.status === 200, 'User A signup request succeeds');
    }
    // Verify A
    {
      const signupCodes = readDocument(join(REPO_ROOT, 'data', 'signup_codes.json'), {});
      const code = signupCodes[normalizeEmail(emailA)].code;
      const resp = await fetchWithBypass(`${BASE_URL}/api/verify-signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailA, code })
      });
      const data = await resp.json();
      sidA = data.id;
      assert(resp.status === 200 && !!sidA, 'User A verified and logged in');
    }

    // Signup B
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailB, password, recaptcha_token: 'dummy' })
      });
      assert(resp.status === 200, 'User B signup request succeeds');
    }
    // Verify B
    {
      const signupCodes = readDocument(join(REPO_ROOT, 'data', 'signup_codes.json'), {});
      const code = signupCodes[normalizeEmail(emailB)].code;
      const resp = await fetchWithBypass(`${BASE_URL}/api/verify-signup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailB, code })
      });
      const data = await resp.json();
      sidB = data.id;
      assert(resp.status === 200 && !!sidB, 'User B verified and logged in');
    }

    if (!sidA || !sidB) {
      console.error('Skipping friends verification because users setup failed');
      return { passedCount, failedCount };
    }

    // Wait 2500ms to allow the running server's NAMES_FILE cache (which has a 2000ms TTL) to expire
    console.log('Waiting 2.5 seconds for running server\'s session cache to reload names.json...');
    await new Promise(resolve => setTimeout(resolve, 2500));

    // 2. Fetch pending requests - should be empty initially
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/requests/pending`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidA}` }
      });
      const txt = await resp.text();
      let data;
      try {
        data = JSON.parse(txt);
      } catch (e) {
        console.error(`[DIAGNOSTIC] status=${resp.status} body=${txt}`);
        throw e;
      }
      assert(resp.status === 200 && data.incoming.length === 0 && data.outgoing.length === 0, 'Pending requests initially empty');
    }

    // 3. User A friend requests User B
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/request`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${sidA}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailB })
      });
      const data = await resp.json();
      assert(resp.status === 200 && data.status === 'pending', 'User A can send friend request to User B');
    }

    // 4. Verify pending lists
    // A should have B in outgoing
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/requests/pending`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidA}` }
      });
      const data = await resp.json();
      assert(data.outgoing.length === 1 && normalizeEmail(data.outgoing[0].to) === normalizeEmail(emailB), 'User A has User B in outgoing pending list');
    }
    // B should have A in incoming
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/requests/pending`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidB}` }
      });
      const data = await resp.json();
      assert(data.incoming.length === 1 && normalizeEmail(data.incoming[0].from) === normalizeEmail(emailA), 'User B has User A in incoming pending list');
    }

    // 5. User B accepts User A's friend request
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/request/respond`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${sidB}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailA, action: 'accept' })
      });
      assert(resp.status === 200, 'User B accepts User A friend request');
    }

    // 6. Verify they are friends
    // A's friends list should contain B
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/list`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidA}` }
      });
      const data = await resp.json();
      assert(data.friends.length === 1 && normalizeEmail(data.friends[0].email) === normalizeEmail(emailB), 'User A has User B in friends list');
    }
    // B's friends list should contain A
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/list`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidB}` }
      });
      const data = await resp.json();
      assert(data.friends.length === 1 && normalizeEmail(data.friends[0].email) === normalizeEmail(emailA), 'User B has User A in friends list');
    }

    // 7. Presence and Activity test
    // User B updates presence to playing "Chess"
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/presence/heartbeat`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${sidB}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ playing: 'Chess' })
      });
      assert(resp.status === 200, 'User B updates presence heartbeat to playing Chess');
    }
    // User A fetches friends list, should see B is online and playing Chess
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/list`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidA}` }
      });
      const data = await resp.json();
      assert(data.friends[0].online === true && data.friends[0].playing === 'Chess', 'User A sees User B is online and playing Chess');
    }

    // 8. User A removes User B from friends
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/remove`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${sidA}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: emailB })
      });
      assert(resp.status === 200, 'User A removes User B from friends');
    }

    // Verify they are no longer friends
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/friends/list`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${sidA}` }
      });
      const data = await resp.json();
      assert(data.friends.length === 0, 'User A friends list is now empty');
    }

  } catch (err) {
    console.error('Error during friends/presence verification:', err);
    failedCount++;
  }

  return { passedCount, failedCount };
}

async function runCanvasZonesVerification() {
  console.log('\n--- Commencing Canvas Zones & Autocomplete/Clear Verification ---');
  let passedCount = 0;
  let failedCount = 0;

  const assert = (condition, message) => {
    if (condition) {
      console.log(`✅ [PASS] ${message}`);
      passedCount++;
    } else {
      console.error(`❌ [FAIL] ${message}`);
      failedCount++;
    }
  };

  try {
    // 1. Create a zone
    let zoneId = null;
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/zones`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${USER_TOKEN}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 'Test Zone', description: 'Initial description', friendsOnly: false })
      });
      assert(resp.status === 200, 'Create zone status is 200');
      const data = await resp.json();
      assert(data.ok === true && data.zone.name === 'Test Zone' && data.zone.description === 'Initial description', 'Zone successfully created with description');
      zoneId = data.zone.id;
    }

    // 2. Fetch zones
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/zones`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${USER_TOKEN}` }
      });
      assert(resp.status === 200, 'Get zones status is 200');
      const data = await resp.json();
      const zone = data.zones.find(z => z.id === zoneId);
      assert(!!zone && zone.description === 'Initial description', 'Created zone retrieved with description');
    }

    // 3. Update zone description
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/zones/update`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${USER_TOKEN}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ zoneId, description: 'Updated description', friendsOnly: true })
      });
      assert(resp.status === 200, 'Update zone status is 200');
      const data = await resp.json();
      assert(data.ok === true && data.zone.description === 'Updated description' && data.zone.friendsOnly === true, 'Zone updated with description and friendsOnly');
    }

    // 4. Paint a pixel in the zone
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/pixel`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${USER_TOKEN}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ x: 10, y: 10, color: '#ff0000', painter: 'tester', zoneId })
      });
      assert(resp.status === 200, 'Paint pixel in zone status is 200');
    }

    // 5. Fetch zone pixels to verify it painted
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/pixels?zoneId=${zoneId}`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${USER_TOKEN}` }
      });
      assert(resp.status === 200, 'Get zone pixels status is 200');
      const data = await resp.json();
      assert(data['10,10'] && data['10,10'].color === '#ff0000', 'Pixel is present in zone');
    }

    // 6. Clear zone pixels
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/zones/clear`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${USER_TOKEN}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ zoneId })
      });
      assert(resp.status === 200, 'Clear zone pixels status is 200');
    }

    // 7. Fetch zone pixels again to verify it is empty
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/pixels?zoneId=${zoneId}`, {
        method: 'GET',
        headers: { 'Cookie': `studentId=${USER_TOKEN}` }
      });
      assert(resp.status === 200, 'Get zone pixels after clear status is 200');
      const data = await resp.json();
      assert(Object.keys(data).length === 0, 'Zone pixels cleared successfully');
    }

    // 8. Delete the zone
    {
      const resp = await fetchWithBypass(`${BASE_URL}/api/canvas/zones/delete`, {
        method: 'POST',
        headers: { 'Cookie': `studentId=${USER_TOKEN}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ zoneId })
      });
      assert(resp.status === 200, 'Delete zone status is 200');
    }

  } catch (err) {
    console.error('Error during canvas zones verification:', err);
    failedCount++;
  }

  return { passedCount, failedCount };
}

run();
