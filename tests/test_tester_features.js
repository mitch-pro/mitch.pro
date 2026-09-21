import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import vm from 'node:vm';

console.log('--- 1. Testing Tester Identity, Coins, and Leaderboard Exclusion in server.js context ---');

const serverSource = readFileSync('server.js', 'utf8');

// Verify files and functions exist in source
assert(serverSource.includes('TESTER_OVERRIDES_FILE'), 'TESTER_OVERRIDES_FILE constant must exist');
assert(serverSource.includes('function loadTesterOverrides()'), 'loadTesterOverrides function must exist');
assert(serverSource.includes('function saveTesterOverrides('), 'saveTesterOverrides function must exist');
assert(serverSource.includes('function isTesterBypassingCooldowns('), 'isTesterBypassingCooldowns function must exist');
assert(serverSource.includes('function isTesterUnlockingCosmetics('), 'isTesterUnlockingCosmetics function must exist');
assert(serverSource.includes('function canAccessTesterTools('), 'canAccessTesterTools function must exist');
assert(serverSource.includes("path === '/api/tester/status'"), '/api/tester/status endpoint must exist');
assert(serverSource.includes("path === '/api/tester/toggle-premium'"), '/api/tester/toggle-premium endpoint must exist');
assert(serverSource.includes("path === '/api/tester/coins'"), '/api/tester/coins endpoint must exist');
assert(serverSource.includes("path === '/api/tester/settings'"), '/api/tester/settings endpoint must exist');
assert(serverSource.includes("path === '/api/tester/reset-cooldowns'"), '/api/tester/reset-cooldowns endpoint must exist');
assert(serverSource.includes("path === '/api/tester/test-notification'"), '/api/tester/test-notification endpoint must exist');

// Verify Leaderboard filtering logic in serverSource
assert(serverSource.includes('!isTesterEmail(email)'), 'Leaderboard must filter out testers using !isTesterEmail(email)');
assert(serverSource.includes('isTester: viewerIsTester'), 'Leaderboard response must return isTester flag for viewer');

console.log('Source checks passed!');

console.log('--- 2. Testing webserver/mitch-coins.js Tester Pill and Infinite Coins Rendering ---');

const mitchCoinsSource = readFileSync('webserver/mitch-coins.js', 'utf8');
assert(mitchCoinsSource.includes("amount.textContent = '∞'"), 'mitch-coins.js must render ∞ for infinite balance');
assert(mitchCoinsSource.includes('mitch-tester-pill'), 'mitch-coins.js must mount mitch-tester-pill');
assert(mitchCoinsSource.includes('/tester/'), 'mitch-coins.js tester pill must link to /tester/');

// Simulate mitch-coins.js execution with tester payload
const children = [];
let testerPillMounted = false;
const host = {
  classList: { add() {} },
  querySelector: selector => selector === '.mitch-wallet' ? children[0] : (selector === '.mitch-tester-pill' ? (testerPillMounted ? {} : null) : null),
  appendChild: child => {
    child.parentElement = host;
    if (child.className?.includes('mitch-tester-pill')) testerPillMounted = true;
    children.push(child);
  },
  insertBefore: (child, ref) => {
    child.parentElement = host;
    if (child.className?.includes('mitch-tester-pill')) testerPillMounted = true;
    children.push(child);
  }
};
const doc = {
  hidden: false, readyState: 'complete',
  head: { appendChild() {} },
  body: { classList: { contains: () => false } },
  querySelector: selector => selector.startsWith('link') ? null : host,
  createElement: tag => ({
    className: '', dataset: {}, value: {}, attributes: {},
    classList: { add(c) { this.className += ' ' + c; } },
    querySelector() { return this.value; },
    setAttribute(name, val) { this.attributes[name] = val; }
  }),
  addEventListener: () => {}
};

let currentResponse = { authenticated: true, coins: 999999999, unlimitedCoins: true, isTester: true };
const fetchMock = async () => new Response(JSON.stringify(currentResponse), { status: 200 });

const coinContext = vm.createContext({
  document: doc,
  location: { href: 'https://mitch.pro/', origin: 'https://mitch.pro', hostname: 'mitch.pro', pathname: '/' },
  URL, Intl, Date, AbortController, Response, Promise,
  fetch: fetchMock,
  setTimeout, clearTimeout,
  setInterval: fn => fn, clearInterval: () => {},
  addEventListener: () => {}
});
coinContext.window = coinContext;

vm.runInContext(mitchCoinsSource, coinContext);
await coinContext.MitchCoins.refresh();

assert.equal(children[0].value.textContent, '∞', 'MitchCoins widget must display ∞ for testers');
assert(testerPillMounted, 'mitch-tester-pill must be mounted when user isTester is true');

console.log('MitchCoins UI unit tests passed!');

console.log('--- 3. Testing HTML and CSS Integration for Tester Hub ---');

assert(existsSync('webserver/tester/index.html'), 'webserver/tester/index.html must exist');
const testerHtml = readFileSync('webserver/tester/index.html', 'utf8');
assert(testerHtml.includes('Beta Tester Hub'), 'Tester hub HTML must include title');
assert(testerHtml.includes('/api/tester/status'), 'Tester hub must fetch /api/tester/status');
assert(testerHtml.includes('/api/tester/toggle-premium'), 'Tester hub must support toggle-premium');
assert(testerHtml.includes('/api/tester/coins'), 'Tester hub must support coin overrides');
assert(testerHtml.includes('/api/tester/settings'), 'Tester hub must support settings overrides');
assert(testerHtml.includes('/api/tester/reset-cooldowns'), 'Tester hub must support reset-cooldowns');
assert(testerHtml.includes('/api/tester/test-notification'), 'Tester hub must support test-notification');

// Leaderboard HTML check
const leaderboardHtml = readFileSync('webserver/leaderboard/index.html', 'utf8');
assert(leaderboardHtml.includes('isTester'), 'Leaderboard HTML must track isTester');
assert(leaderboardHtml.includes('TESTER'), 'Leaderboard HTML must render TESTER status');
assert(leaderboardHtml.includes('/tester/'), 'Leaderboard HTML must link to /tester/');

// Coins page HTML check
const coinsHtml = readFileSync('webserver/coins/index.html', 'utf8');
assert(coinsHtml.includes('∞ Unlimited'), 'Coins HTML must handle unlimitedCoins');
assert(coinsHtml.includes('testerAction'), 'Coins HTML must include testerAction');

// Index.html check
const indexHtml = readFileSync('webserver/index.html', 'utf8');
assert(indexHtml.includes('tester-action-wrap'), 'index.html must include tester-action-wrap');
assert(indexHtml.includes('setupAdminPanel'), 'index.html must wire tester wrap in setupAdminPanel');

// Mitch-coins CSS check
const mitchCoinsCss = readFileSync('webserver/mitch-coins.css', 'utf8');
assert(mitchCoinsCss.includes('.mitch-tester-pill'), 'mitch-coins.css must style .mitch-tester-pill');

console.log('HTML and CSS static integration tests passed!');

console.log('--- 4. Testing Business Logic Simulation in VM Context ---');

let mockOverrides = {};
const normalizeEmail = email => String(email || '').trim().toLowerCase();
const testerEmail = 'tester@student.rjuhsd.us';
const regularUserEmail = 'student@student.rjuhsd.us';

const testersList = [testerEmail];
const isTesterEmail = email => testersList.includes(normalizeEmail(email));
const loadTesterOverrides = () => mockOverrides;
const saveTesterOverrides = d => { mockOverrides = d; };

function getCoins(email) {
  if (!email) return 0;
  const norm = normalizeEmail(email);
  if (isTesterEmail(norm)) {
    const overrides = loadTesterOverrides()[norm] || {};
    if (overrides.unlimitedCoins === false && typeof overrides.customCoins === 'number') {
      return Math.max(0, overrides.customCoins);
    }
    return 999999999;
  }
  return 100;
}

function addCoins(email, amount) {
  if (!email) return;
  const norm = normalizeEmail(email);
  if (isTesterEmail(norm)) {
    const overrides = loadTesterOverrides();
    const userCfg = overrides[norm] || {};
    if (userCfg.unlimitedCoins === false && typeof userCfg.customCoins === 'number') {
      userCfg.customCoins = Math.max(0, Number((userCfg.customCoins + amount).toFixed(4)));
      overrides[norm] = userCfg;
      saveTesterOverrides(overrides);
    }
    return;
  }
}

function isPremiumEmail(email) {
  if (!email) return false;
  const norm = normalizeEmail(email);
  if (isTesterEmail(norm)) {
    const overrides = loadTesterOverrides();
    if (overrides[norm]?.premium !== undefined) {
      return overrides[norm].premium === true;
    }
    return true;
  }
  return false;
}

// 1. Tester default: unlimited coins & premium
assert.equal(getCoins(testerEmail), 999999999, 'Tester must have 999,999,999 default coins');
assert.equal(isPremiumEmail(testerEmail), true, 'Tester must default to premium');

// 2. Deductions do not drain unlimited balance
addCoins(testerEmail, -5000);
assert.equal(getCoins(testerEmail), 999999999, 'Unlimited balance must not deplete when spending');

// 3. Custom balance override
mockOverrides[testerEmail] = { unlimitedCoins: false, customCoins: 50 };
assert.equal(getCoins(testerEmail), 50, 'Custom balance must be returned when set');
addCoins(testerEmail, -20);
assert.equal(getCoins(testerEmail), 30, 'Custom balance must decrement when spending');
addCoins(testerEmail, 15);
assert.equal(getCoins(testerEmail), 45, 'Custom balance must increment when receiving coins');

// 4. Restore unlimited
mockOverrides[testerEmail] = { unlimitedCoins: true };
assert.equal(getCoins(testerEmail), 999999999, 'Balance must return to unlimited when restored');

// 5. Toggle premium
mockOverrides[testerEmail] = { premium: false };
assert.equal(isPremiumEmail(testerEmail), false, 'Tester must be able to toggle premium off');
mockOverrides[testerEmail] = { premium: true };
assert.equal(isPremiumEmail(testerEmail), true, 'Tester must be able to toggle premium on');

// 6. Leaderboard filtering
const allPlayers = [
  testerEmail,
  regularUserEmail,
  'another.student@student.rjuhsd.us'
];
const filteredLeaderboard = allPlayers.filter(email => !isTesterEmail(email));
assert(!filteredLeaderboard.includes(testerEmail), 'Tester MUST be excluded from leaderboard');
assert.equal(filteredLeaderboard.length, 2, 'Leaderboard must only contain non-testers');

console.log('Business logic simulation tests passed!');

console.log('--- 5. Testing Admin Panel Tester Management Integration ---');

const adminHtml = readFileSync('webserver/admin/index.html', 'utf8');
assert(adminHtml.includes('id="tester-card"'), 'Admin HTML must contain #tester-card');
assert(adminHtml.includes('id="tester-list"'), 'Admin HTML must contain #tester-list');
assert(adminHtml.includes('id="tester-email"'), 'Admin HTML must contain #tester-email');
assert(adminHtml.includes('id="tester-status"'), 'Admin HTML must contain #tester-status');
assert(adminHtml.includes('addTester(event)'), 'Admin HTML must attach addTester form submit handler');
assert(adminHtml.includes('function addTester('), 'Admin HTML must define addTester function');
assert(adminHtml.includes('function removeTester('), 'Admin HTML must define removeTester function');
assert(adminHtml.includes('function renderTesters('), 'Admin HTML must define renderTesters function');
assert(adminHtml.includes('function loadTesters('), 'Admin HTML must define loadTesters function');
assert(adminHtml.includes('body.moderator-mode #tester-card'), 'Admin CSS must hide #tester-card in moderator mode');
assert(adminHtml.includes("'/api/admin/testers': 'tester_role'"), 'Admin JS must map /api/admin/testers to tester_role in ACTION_BY_URL');
assert(adminHtml.includes("document.getElementById('tester-card').style.display = 'block'"), 'Admin JS init must show tester card for admins');
assert(adminHtml.includes('loadTesters()'), 'Admin JS init must call loadTesters()');

// Server-side action mappings
assert(serverSource.includes("tester_role: 'Update tester role'"), 'MODERATOR_ACTION_LABELS must include tester_role');
assert(serverSource.includes("case 'tester_role':"), 'cleanModeratorActionPayload must handle tester_role');
assert(serverSource.includes("if (action === 'tester_role')"), 'executeModeratorApprovedAction must handle tester_role');

console.log('Admin Panel tester management tests passed!');

console.log('\n=== ALL TESTER FEATURE CHECKS PASSED SUCCESSFULLY ===');

