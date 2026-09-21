import {
  canAccessVmRecord,
  validateDesktopSession,
  VmOperationGate,
  VM_MAX_CONCURRENT_RUNNING,
  VM_DAILY_MAX_SECONDS,
  VM_EXTENSION_COOLDOWN_MS,
  VM_COOLDOWN_DURATION_MS,
  VM_OFFPAGE_INACTIVITY_MS,
  VM_ADMIN_OFFPAGE_INACTIVITY_MS,
  VM_DEFAULT_CPU_CORES,
  VM_DEFAULT_MEMORY_MB,
  VM_DEFAULT_BALLOON_MB,
  VM_DEFAULT_DISK_GB,
  VM_MAX_UPGRADE_CPU_CORES,
  VM_MAX_UPGRADE_MEMORY_MB,
  VM_MAX_UPGRADE_DISK_GB,
  VM_FLEET_MAX_CORES,
  VM_FLEET_MAX_MEMORY_MB,
  VM_UPGRADE_CATALOG,
  checkFleetResourceCapacity,
  getRemainingDailyVmSeconds,
  isDailyVmLimitReached,
  getVmDayKey,
  isEligibleForFreeVm,
  canUserExtend,
  computeCooldownRemaining,
  isVmInactive,
  isVmAdminAccessAllowed,
  isVmAdminAccessRequested,
  shouldNotifyCapacityAlert,
  formatCapacityFullAlert,
  formatAdminUsageNotice,
  formatAdminAccessRequest,
  formatUptimeDuration,
} from '../lib/vm_security.js';
import {
  recordVmUsageSample,
  listVmUsageSamples,
  getVmUsageTimeline,
  pruneOldVmUsageSamples,
} from '../lib/data_store.js';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { ProxmoxDesktopService, ProxmoxServiceError } from '../lib/proxmox_desktop.js';

function assert(condition, message) { if (!condition) throw new Error(message); }
const vmA = { id: 'vm-a', ownerEmail: 'a@example.com', vmid: 301, node: 'node-a', guestType: 'qemu' };
const vmB = { id: 'vm-b', ownerEmail: 'b@example.com', vmid: 302, node: 'node-a', guestType: 'qemu' };
const actorA = { sid: 'sid-a', email: 'a@example.com', isAdmin: false };

assert(canAccessVmRecord(vmA, actorA), 'owner must access their computer');
assert(!canAccessVmRecord(vmB, actorA), 'User A must not access User B computer');
assert(canAccessVmRecord(vmB, { ...actorA, isAdmin: true }), 'admins must be able to support assigned computers');
assert(!canAccessVmRecord(null, actorA), 'nonexistent computer must be denied');

const vmAdmin2 = { id: 'vm-c', ownerEmail: 'admin2@example.com', vmid: 303, node: 'node-a', guestType: 'qemu' };
const actorAdmin1 = { sid: 'sid-admin1', email: 'admin1@example.com', isAdmin: true };
const isTestAdmin = email => email === 'admin1@example.com' || email === 'admin2@example.com';

assert(!canAccessVmRecord(vmAdmin2, actorAdmin1, isTestAdmin), 'admin must not access another admin computer');
assert(canAccessVmRecord(vmAdmin2, { ...actorAdmin1, email: 'admin2@example.com' }, isTestAdmin), 'admin owner must access their own computer');
assert(canAccessVmRecord(vmB, actorAdmin1, isTestAdmin), 'admin must access regular user computer');

const vmAdminWithFlag = { id: 'vm-d', ownerEmail: 'admin2@example.com', ownerIsAdmin: true, vmid: 304, node: 'node-a', guestType: 'qemu' };
assert(!canAccessVmRecord(vmAdminWithFlag, actorAdmin1), 'admin must not access another admin computer with ownerIsAdmin flag');

// Admin grant requirement tests
assert(!canAccessVmRecord(vmB, actorAdmin1, { isAdminEmail: isTestAdmin, requireAdminGrant: true }), 'admin must not access user computer without grant');
assert(!canAccessVmRecord({ ...vmB, adminAccessAllowed: false }, actorAdmin1, { isAdminEmail: isTestAdmin, requireAdminGrant: true }), 'admin must not access user computer with adminAccessAllowed: false');
assert(canAccessVmRecord({ ...vmB, adminAccessAllowed: true }, actorAdmin1, { isAdminEmail: isTestAdmin, requireAdminGrant: true }), 'admin must access user computer when adminAccessAllowed: true');
assert(canAccessVmRecord(vmB, actorAdmin1, { isAdminEmail: isTestAdmin, requireAdminGrant: true, isGrantAllowed: id => id === vmB.id }), 'admin must access user computer when isGrantAllowed returns true');
assert(!canAccessVmRecord(vmB, actorAdmin1, { isAdminEmail: isTestAdmin, requireAdminGrant: true, isGrantAllowed: () => false }), 'admin must not access user computer when isGrantAllowed returns false');
assert(canAccessVmRecord(vmA, actorA, { requireAdminGrant: true }), 'owner can access their own computer regardless of grant setting');

const sessionAdminToAdmin = { sid: 'sid-admin1', actorEmail: 'admin1@example.com', recordId: 'vm-c', vmid: 303, node: 'node-a', expiresAt: 2000, used: false };
assert(validateDesktopSession(sessionAdminToAdmin, actorAdmin1, vmAdmin2, 1000, isTestAdmin).status === 403, 'session from admin to another admin computer must be forbidden');

const sessionAdminToUser = { sid: 'sid-admin1', actorEmail: 'admin1@example.com', recordId: 'vm-b', vmid: 302, node: 'node-a', expiresAt: 2000, used: false };
assert(validateDesktopSession(sessionAdminToUser, actorAdmin1, vmB, 1000, { isAdminEmail: isTestAdmin, requireAdminGrant: true }).status === 403, 'session from admin without grant must be 403');
assert(validateDesktopSession(sessionAdminToUser, actorAdmin1, { ...vmB, adminAccessAllowed: true }, 1000, { isAdminEmail: isTestAdmin, requireAdminGrant: true }).ok, 'session from admin with grant must be ok');

const goodSession = { sid: 'sid-a', actorEmail: 'a@example.com', recordId: 'vm-a', vmid: 301, node: 'node-a', expiresAt: 2000, used: false };
assert(validateDesktopSession(goodSession, actorA, vmA, 1000).ok, 'valid desktop connection must be accepted');
assert(validateDesktopSession(goodSession, actorA, vmA, 2001).status === 401, 'expired desktop connection must be rejected');
assert(validateDesktopSession({ ...goodSession, used: true }, actorA, vmA, 1000).status === 401, 'reused desktop connection must be rejected');
assert(validateDesktopSession(goodSession, actorA, vmB, 1000).status === 403, 'WebSocket ownership mismatch must be rejected');

const gate = new VmOperationGate(3000);
assert(gate.acquire('vm-a', 'restart', 1000), 'first power request must be accepted');
assert(!gate.acquire('vm-a', 'restart', 1100), 'repeated power request must be rejected');
gate.release('vm-a', 1100);
assert(!gate.acquire('vm-a', 'restart', 4000), 'request must remain blocked during cooldown');
assert(gate.acquire('vm-a', 'restart', 4101), 'request must be accepted after cooldown');

const stoppedService = new ProxmoxDesktopService({ host: 'localhost', node: 'node-a', legacyToken: 'token' });
stoppedService.getStatus = async () => ({ state: 'stopped' });
let stoppedError = null;
try { await stoppedService.createConsole(vmA); } catch (error) { stoppedError = error; }
assert(stoppedError instanceof ProxmoxServiceError && stoppedError.code === 'STOPPED', 'stopped computer must not create a console');

const failedService = new ProxmoxDesktopService({ host: 'localhost', node: 'node-a', legacyToken: 'token' });
const originalFetch = globalThis.fetch;
globalThis.fetch = async () => new Response(JSON.stringify({ errors: { auth: 'secret upstream detail' } }), { status: 500, headers: { 'Content-Type': 'application/json' } });
let failedError = null;
try { await failedService.listGuests(); } catch (error) { failedError = error; }
finally { globalThis.fetch = originalFetch; }
assert(failedError instanceof ProxmoxServiceError && failedError.code === 'UPSTREAM_REJECTED', 'failed Proxmox API call must be normalized');
assert(!failedError.message.includes('secret upstream detail'), 'raw Proxmox errors must not escape');

for (const invalid of [null, '', 'abc', 99, -1]) {
  let error = null;
  try { failedService.assertVmid(invalid); } catch (caught) { error = caught; }
  assert(error?.code === 'INVALID_VM', `invalid VM ID ${String(invalid)} must be rejected`);
}

const guestService = new ProxmoxDesktopService({ host: 'localhost', node: 'node-a', legacyToken: 'token' });
const guestCalls = [];
guestService.request = async (method, path, params) => {
  guestCalls.push({ method, path, params });
  if (path.endsWith('/agent/exec')) return { pid: 42 };
  if (path.includes('/agent/exec-status')) return { exited: 1, exitcode: 0 };
  throw new Error(`Unexpected guest-agent request: ${method} ${path}`);
};
await guestService.guestExec(301, ['/usr/bin/id', 'desktop']);
assert(JSON.stringify(guestCalls[0].params.command) === '["/usr/bin/id","desktop"]', 'guest commands must use the Proxmox repeated-parameter array encoding');

// --- Guest Agent Ping & Provisioning Tests ---
const pingCalls = [];
const pingService = new ProxmoxDesktopService({ host: 'localhost', node: 'node-a', legacyToken: 'token' });
pingService.request = async (method, path, params) => {
  pingCalls.push({ method, path, params });
  if (method === 'POST' && path.endsWith('/agent/ping')) return {};
  throw new Error(`Unexpected request: ${method} ${path}`);
};
await pingService.waitForGuestAgent(301, 5000);
assert(pingCalls.length === 1 && pingCalls[0].method === 'POST', 'guest agent ping must use POST method (GET returns 501 on Proxmox VE)');
assert(pingCalls[0].path === '/nodes/node-a/qemu/301/agent/ping', 'guest agent ping must target the correct VM node and vmid');

let pingTimeoutError = null;
const failingPingService = new ProxmoxDesktopService({ host: 'localhost', node: 'node-a', legacyToken: 'token' });
failingPingService.request = async () => { throw new Error('Unreachable'); };
try { await failingPingService.waitForGuestAgent(301, 100); } catch (e) { pingTimeoutError = e; }
assert(pingTimeoutError instanceof ProxmoxServiceError && pingTimeoutError.code === 'GUEST_SETUP_FAILED', 'unreachable guest agent ping must time out with GUEST_SETUP_FAILED');

let setupCommand = null;
guestService.waitForGuestAgent = async () => {};
guestService.guestExec = async (_vmid, command, inputData) => { setupCommand = { command, inputData }; };
await guestService.enableFriendlyDesktopLogin(301, 'desktop');
assert(setupCommand.command.join(' ') === '/bin/sh -s', 'desktop setup must run through a fixed shell entrypoint');
assert(setupCommand.inputData.includes('user=desktop') && setupCommand.inputData.includes('AutomaticLogin=$user'), 'desktop setup must enable automatic graphical login for the validated user');
assert(setupCommand.inputData.includes('idle-delay 0'), 'desktop setup must keep browser desktops awake');
assert(setupCommand.inputData.includes('lock-enabled false'), 'desktop setup must not strand users at an idle lock screen');
assert(setupCommand.inputData.includes('getent group "$user"'), 'desktop setup must check for existing groups before useradd');

// Test password provisioning in enableFriendlyDesktopLogin
await guestService.enableFriendlyDesktopLogin(301, 'studentuser', 'SuperSecret123!');
assert(setupCommand.inputData.includes('chpasswd'), 'desktop setup must set user password when provided');
const expectedB64 = Buffer.from('SuperSecret123!', 'utf8').toString('base64');
assert(setupCommand.inputData.includes(`pass_b64="${expectedB64}"`), 'desktop setup must safely base64 encode the user password');
assert(setupCommand.inputData.includes('systemd-networkd-wait-online.service'), 'desktop setup must mask networkd-wait-online service');
assert(setupCommand.inputData.includes('modprobe virtio_rng'), 'desktop setup must ensure virtio-rng kernel module is loaded');

let unsafeLoginError = null;
try { await guestService.enableFriendlyDesktopLogin(301, 'desktop\nroot'); } catch (error) { unsafeLoginError = error; }
assert(unsafeLoginError?.code === 'INVALID_DESKTOP_LOGIN', 'desktop login setup must reject unsafe usernames');

// --- Desktop Password Validation (any non-empty length) ---
const login1 = guestService.validateDesktopLogin('studentuser', 'x');
assert(login1.username === 'studentuser' && login1.password === 'x', '1-character password must be accepted');
const loginLong = guestService.validateDesktopLogin('studentuser', 'A'.repeat(4096));
assert(loginLong.password.length === 4096, 'Long passwords must be accepted without an artificial maximum');

let emptyPassError = null;
try { guestService.validateDesktopLogin('studentuser', ''); } catch (e) { emptyPassError = e; }
assert(emptyPassError?.code === 'INVALID_DESKTOP_LOGIN', 'Empty passwords must be rejected');

let badCharError = null;
try { guestService.validateDesktopLogin('studentuser', 'password\n123'); } catch (e) { badCharError = e; }
assert(badCharError?.code === 'INVALID_DESKTOP_LOGIN', 'password with newline must be rejected');

// --- Free VM Eligibility Policy ---
assert(isEligibleForFreeVm('john.doe@student.rjuhsd.us'), '@student.rjuhsd.us email must be eligible for free VM');
assert(isEligibleForFreeVm('student@student.mitch.pro'), '@student.mitch.pro email must be eligible for free VM');
assert(!isEligibleForFreeVm('user@gmail.com'), 'normal public email must not be eligible by default');
assert(!isEligibleForFreeVm('teacher@rjuhsd.us'), 'staff @rjuhsd.us email must not be eligible unless premium or admin');
assert(isEligibleForFreeVm('user@gmail.com', { isPremium: true }), 'premium users must be eligible for free VM');
assert(isEligibleForFreeVm('admin@mitch.pro', { isAdmin: true }), 'admins must always be eligible for free VM');
assert(!isEligibleForFreeVm(''), 'empty email must not be eligible');
assert(!isEligibleForFreeVm(null), 'null email must not be eligible');

// --- 24-hour Extension Limit (1 extension per day) ---
const now = Date.now();
assert(canUserExtend(null, { isAdmin: false, now }), 'user with no prior extension must be allowed to extend');
assert(!canUserExtend(now - (60 * 1000), { isAdmin: false, now }), 'user with extension 1 minute ago must be denied');
assert(!canUserExtend(now - (23 * 60 * 60 * 1000), { isAdmin: false, now }), 'user with extension 23 hours ago must be denied');
assert(canUserExtend(now - (24 * 60 * 60 * 1000), { isAdmin: false, now }), 'user with extension 24 hours ago must be allowed');
assert(canUserExtend(now - (25 * 60 * 60 * 1000), { isAdmin: false, now }), 'user with extension 25 hours ago must be allowed');
assert(canUserExtend(now - 1000, { isAdmin: true, now }), 'admin can always extend regardless of cooldown');

// --- 30-minute Cooldown After Session End ---
assert(computeCooldownRemaining(now + 1800 * 1000, { isAdmin: false, now }) === 1800, 'cooldown should report 1800 seconds remaining');
assert(computeCooldownRemaining(now + 60 * 1000, { isAdmin: false, now }) === 60, 'cooldown should report 60 seconds remaining');
assert(computeCooldownRemaining(now - 1000, { isAdmin: false, now }) === 0, 'expired cooldown should report 0 seconds remaining');
assert(computeCooldownRemaining(null, { isAdmin: false, now }) === 0, 'no cooldown should report 0 seconds remaining');
assert(computeCooldownRemaining(now + 1800 * 1000, { isAdmin: true, now }) === 0, 'admin should have 0 cooldown remaining');

// --- 10-minute Off-Page Inactivity Detection (User) & 30-minute Detection (Admin) ---
assert(!isVmInactive(now - (9 * 60 * 1000), { now }), 'activity 9 minutes ago must not be considered inactive');
assert(isVmInactive(now - (10 * 60 * 1000), { now }), 'activity 10 minutes ago must be considered inactive');
assert(isVmInactive(now - (15 * 60 * 1000), { now }), 'activity 15 minutes ago must be considered inactive');
assert(!isVmInactive(null, { now }), 'null presence must not be marked inactive');
assert(!isVmInactive(now - (29 * 60 * 1000), { now, isAdmin: true }), 'admin activity 29 minutes ago must not be inactive');
assert(isVmInactive(now - (30 * 60 * 1000), { now, isAdmin: true }), 'admin activity 30 minutes ago must be inactive');
assert(isVmInactive(now - (35 * 60 * 1000), { now, isAdmin: true }), 'admin activity 35 minutes ago must be inactive');
assert(!isVmInactive(now - (15 * 60 * 1000), { now, isAdmin: true }), 'admin activity 15 minutes ago must not be inactive');

// --- Fleet Capacity Limit ---
assert(VM_FLEET_MAX_CORES === 36, 'max fleet CPU cores must be 36');
assert(VM_FLEET_MAX_MEMORY_MB === 98304, 'max fleet memory must be 96 GB (98304 MB)');
assert(VM_COOLDOWN_DURATION_MS === 30 * 60 * 1000, 'cooldown duration must be 30 minutes');
assert(VM_EXTENSION_COOLDOWN_MS === 24 * 60 * 60 * 1000, 'extension cooldown must be 24 hours');
assert(VM_OFFPAGE_INACTIVITY_MS === 10 * 60 * 1000, 'offpage inactivity timeout must be 10 minutes');
assert(VM_ADMIN_OFFPAGE_INACTIVITY_MS === 30 * 60 * 1000, 'admin offpage inactivity timeout must be 30 minutes');

// --- VM Defaults (2 Cores, 4 GB RAM, 64 GB Disk) and Upgrades (Up to 6 Cores, 16 GB RAM, 256 GB Disk) ---
assert(VM_DAILY_MAX_SECONDS === 6 * 3600, 'daily max VM seconds must be 6 hours (21600 seconds)');
assert(VM_DEFAULT_CPU_CORES === 2, 'default CPU cores must be 2');
assert(VM_DEFAULT_MEMORY_MB === 4096, 'default memory must be 4096 MB (4 GB)');
assert(VM_DEFAULT_BALLOON_MB === 1024, 'default balloon memory must be 1024 MB (1 GB)');
assert(VM_DEFAULT_DISK_GB === 64, 'default disk must be 64 GB');
assert(VM_MAX_UPGRADE_CPU_CORES === 6, 'max upgrade CPU cores must be 6');
assert(VM_MAX_UPGRADE_MEMORY_MB === 16384, 'max upgrade memory must be 16384 MB (16 GB)');
assert(VM_MAX_UPGRADE_DISK_GB === 128, 'max upgrade disk must be 128 GB');

// --- Fleet Resource Capacity Check ---
const cap1 = checkFleetResourceCapacity(30, 80 * 1024, 6, 16 * 1024);
assert(cap1.ok === true, '36 cores and 96 GB RAM must fit in fleet capacity');
assert(cap1.totalCores === 36, 'total cores must be 36');
assert(cap1.totalMemoryMb === 98304, 'total memory must be 98304 MB');

const capOverCores = checkFleetResourceCapacity(36, 64 * 1024, 2, 4 * 1024);
assert(capOverCores.ok === false, '38 cores must exceed 36 cores fleet limit');

const capOverMem = checkFleetResourceCapacity(20, 96 * 1024, 2, 4 * 1024);
assert(capOverMem.ok === false, '100 GB RAM must exceed 96 GB fleet limit');

// --- Upgrade Catalog and Differential Pricing ---
assert(Array.isArray(VM_UPGRADE_CATALOG.cpu), 'catalog must have cpu tiers');
assert(Array.isArray(VM_UPGRADE_CATALOG.ram), 'catalog must have ram tiers');
assert(Array.isArray(VM_UPGRADE_CATALOG.disk), 'catalog must have disk tiers');
assert(Array.isArray(VM_UPGRADE_CATALOG.session), 'catalog must have session tiers');

const cpuMax = VM_UPGRADE_CATALOG.cpu[VM_UPGRADE_CATALOG.cpu.length - 1];
assert(cpuMax.value === 6, 'max cpu tier in catalog must be 6 cores');
const ramMax = VM_UPGRADE_CATALOG.ram[VM_UPGRADE_CATALOG.ram.length - 1];
assert(ramMax.value === 16384, 'max ram tier in catalog must be 16384 MB (16 GB)');
const diskMax = VM_UPGRADE_CATALOG.disk[VM_UPGRADE_CATALOG.disk.length - 1];
assert(diskMax.value === 128, 'max disk tier in catalog must be 128 GB');
const sessionMax = VM_UPGRADE_CATALOG.session[VM_UPGRADE_CATALOG.session.length - 1];
assert(sessionMax.value === 86400, 'max session tier in catalog must be 86400s (24h unlimited)');

// Differential pricing verification
function calcCost(cat, fromVal, toVal) {
  const fromTier = VM_UPGRADE_CATALOG[cat].find(t => t.value === fromVal) || { cost: 0 };
  const toTier = VM_UPGRADE_CATALOG[cat].find(t => t.value === toVal) || { cost: 0 };
  return Math.max(0, toTier.cost - fromTier.cost);
}
assert(calcCost('cpu', 2, 4) === 400, '2 -> 4 cores should cost 400 coins');
assert(calcCost('cpu', 4, 6) === 400, '4 -> 6 cores should cost 400 coins (differential)');
assert(calcCost('cpu', 2, 6) === 800, '2 -> 6 cores should cost 800 coins');
assert(calcCost('ram', 4096, 16384) === 1200, '4GB -> 16GB should cost 1200 coins');
assert(calcCost('ram', 8192, 16384) === 800, '8GB -> 16GB should cost 800 coins');
assert(calcCost('disk', 64, 96) === 300, '64GB -> 96GB should cost 300 coins');
assert(calcCost('disk', 64, 128) === 1200, '64GB -> 128GB should cost 1200 coins');
assert(calcCost('session', 21600, 86400) === 1800, '6h -> 24h should cost 1800 coins');

// --- Daily Max and Admin/Session Upgraded Exemption ---
assert(getRemainingDailyVmSeconds(0) === 21600, '0 used seconds must leave 21600 seconds remaining');
assert(getRemainingDailyVmSeconds(3600) === 18000, '1 hour used must leave 5 hours remaining');
assert(getRemainingDailyVmSeconds(21600) === 0, '6 hours used must leave 0 seconds remaining');
assert(getRemainingDailyVmSeconds(25000) === 0, 'over 6 hours used must leave 0 seconds remaining');
assert(getRemainingDailyVmSeconds(21600, { isAdmin: true }) === Infinity, 'admin must have Infinity remaining seconds');
assert(getRemainingDailyVmSeconds(50000, { isAdmin: true }) === Infinity, 'admin must have Infinity remaining seconds regardless of usage');
assert(getRemainingDailyVmSeconds(50000, { dailyMaxSeconds: 86400 }) === Infinity, 'unlimited 24h session upgrade must have Infinity remaining seconds');
assert(getRemainingDailyVmSeconds(20000, { dailyMaxSeconds: 36000 }) === 16000, '10h session with 20000s used must leave 16000s remaining');

assert(!isDailyVmLimitReached(0), '0 used must not reach daily limit');
assert(!isDailyVmLimitReached(21599), '21599s used must not reach daily limit');
assert(isDailyVmLimitReached(21600), '21600s used must reach daily limit');
assert(isDailyVmLimitReached(30000), '30000s used must reach daily limit');
assert(!isDailyVmLimitReached(21600, { isAdmin: true }), 'admin must not be subject to daily limit');
assert(!isDailyVmLimitReached(99999, { isAdmin: true }), 'admin must not be subject to daily limit even with high usage');
assert(!isDailyVmLimitReached(99999, { dailyMaxSeconds: 86400 }), 'unlimited session upgrade must not be subject to daily limit');

assert(getVmDayKey(new Date('2026-09-17T12:00:00Z').getTime()) === '2026-09-17', 'getVmDayKey must return YYYY-MM-DD');

// --- VM Boot Optimizations (VirtIO RNG, host CPU, VirtIO VGA) ---
const optService = new ProxmoxDesktopService({ host: 'localhost', node: 'node-a', legacyToken: 'token', templateVmids: [9010] });
const optPuts = [];
let mockConfig = { cpu: 'kvm64', vga: 'std' };
optService.request = async (method, path, params) => {
  if (method === 'GET' && path.endsWith('/config')) return { ...mockConfig };
  if (method === 'PUT' && path.endsWith('/config')) {
    optPuts.push(params);
    mockConfig = { ...mockConfig, ...params };
    return {};
  }
  if (method === 'POST' && path.endsWith('/status/start')) return { upid: 'UPID:start' };
  throw new Error(`Unexpected request: ${method} ${path}`);
};

const applied = await optService.ensureOptimizedVmConfig(401);
assert(applied.cpu === 'host', 'optimization must set cpu to host');
assert(applied.rng0 === 'source=/dev/urandom', 'optimization must set rng0 to /dev/urandom');
assert(applied.vga === 'std,memory=64', 'optimization must set vga to std,memory=64');
assert(optPuts.length === 1, 'PUT config must have been invoked once');

// Redundant call should use cache and skip PUT
const cachedApplied = await optService.ensureOptimizedVmConfig(401);
assert(cachedApplied === null, 'cached call must return null without re-querying');
assert(optPuts.length === 1, 'PUT config must not be invoked again for cached VMID');

// Power start on unoptimized VM should trigger ensureOptimizedVmConfig
optService.optimizedVmids.clear();
mockConfig = { cpu: 'kvm64' };
await optService.power({ vmid: 402, node: 'node-a', guestType: 'qemu' }, 'start');
assert(optPuts.length === 2, 'power start must trigger config optimization on QEMU guest');
assert(optPuts[1].cpu === 'host' && optPuts[1].rng0 === 'source=/dev/urandom' && optPuts[1].vga === 'std,memory=64', 'power start must apply all boot optimizations');

// --- Admin Access Grants and Notifications Unit Tests ---
const testGrants = {
  'vm-101': { allowed: true, requested: false, ownerEmail: 'user1@example.com' },
  'vm-102': { allowed: false, requested: true, ownerEmail: 'user2@example.com' },
};
assert(isVmAdminAccessAllowed('vm-101', testGrants), 'vm-101 must have admin access allowed');
assert(!isVmAdminAccessAllowed('vm-102', testGrants), 'vm-102 must not have admin access allowed');
assert(!isVmAdminAccessAllowed('vm-999', testGrants), 'unlisted VM must not have admin access allowed');
assert(!isVmAdminAccessAllowed(null, testGrants), 'null VMID must not have admin access allowed');

assert(!isVmAdminAccessRequested('vm-101', testGrants), 'vm-101 must not have access requested');
assert(isVmAdminAccessRequested('vm-102', testGrants), 'vm-102 must have access requested');
assert(!isVmAdminAccessRequested('vm-999', testGrants), 'unlisted VM must not have access requested');

// --- Capacity Alert Debounce and Formatting ---
const lastCapacityMap = new Map();
const t0 = 1000000;
assert(shouldNotifyCapacityAlert('user@example.com', t0, lastCapacityMap, 60000), 'first capacity attempt must notify');
assert(!shouldNotifyCapacityAlert('user@example.com', t0 + 10000, lastCapacityMap, 60000), 'attempt within 60s cooldown must not notify');
assert(!shouldNotifyCapacityAlert('USER@EXAMPLE.COM', t0 + 30000, lastCapacityMap, 60000), 'case-insensitive attempt within cooldown must not notify');
assert(shouldNotifyCapacityAlert('other@example.com', t0 + 10000, lastCapacityMap, 60000), 'different user must notify');
assert(shouldNotifyCapacityAlert('user@example.com', t0 + 60001, lastCapacityMap, 60000), 'attempt after 60s cooldown must notify');

const capAlert = formatCapacityFullAlert('student@rjuhsd.us', 'start computer My PC', 6);
assert(capAlert.title === 'VM Capacity Alert', 'title must be VM Capacity Alert');
assert(capAlert.message.includes('6/6'), 'message must indicate 6/6 capacity full');
assert(capAlert.message.includes('student@rjuhsd.us'), 'message must include user email');
assert(capAlert.priority === 'high', 'priority must be high');

const usageNotice = formatAdminUsageNotice('admin@mitch.pro', 'My PC', 'power-restart');
assert(usageNotice.title === 'Admin Used Your Computer', 'title must be Admin Used Your Computer');
assert(usageNotice.message.includes('admin@mitch.pro'), 'message must include admin email');
assert(usageNotice.message.includes('power-restart'), 'message must include operation');

const reqNotice = formatAdminAccessRequest('admin@mitch.pro', 'My PC');
assert(reqNotice.title === 'Admin Access Request', 'title must be Admin Access Request');
assert(reqNotice.message.includes('admin@mitch.pro'), 'message must include admin email');

// --- Uptime Duration Formatting ---
assert(formatUptimeDuration(0) === '0m', '0 seconds must format to 0m');
assert(formatUptimeDuration(59) === '0m', '59 seconds must format to 0m');
assert(formatUptimeDuration(60) === '1m', '60 seconds must format to 1m');
assert(formatUptimeDuration(150) === '2m', '150 seconds must format to 2m');
assert(formatUptimeDuration(3600) === '1h 0m', '3600 seconds must format to 1h 0m');
assert(formatUptimeDuration(7320) === '2h 2m', '7320 seconds must format to 2h 2m');
assert(formatUptimeDuration(-100) === '0m', 'negative duration must format to 0m');

// --- VM Usage Logging and Timeline (Not Anonymous) ---
const testDayKey = `test-${Date.now()}`;
recordVmUsageSample({
  ts: new Date('2026-09-19T02:15:00Z').getTime(),
  dayKey: testDayKey,
  hour: 2,
  minute: 15,
  ownerEmail: 'alice@example.com',
  vmRecordId: 'vm-alice-1',
  vmid: 501,
  vmName: "Alice's Machine",
  uptimeSeconds: 1200,
  activeUsers: 'alice@example.com',
  isRunning: 1,
});

recordVmUsageSample({
  ts: new Date('2026-09-19T02:15:00Z').getTime(),
  dayKey: testDayKey,
  hour: 2,
  minute: 15,
  ownerEmail: 'bob@example.com',
  vmRecordId: 'vm-bob-1',
  vmid: 502,
  vmName: "Bob's Workstation",
  uptimeSeconds: 3600,
  activeUsers: '',
  isRunning: 1,
});

recordVmUsageSample({
  ts: new Date('2026-09-19T02:45:00Z').getTime(),
  dayKey: testDayKey,
  hour: 2,
  minute: 45,
  ownerEmail: 'bob@example.com',
  vmRecordId: 'vm-bob-1',
  vmid: 502,
  vmName: "Bob's Workstation",
  uptimeSeconds: 5400,
  activeUsers: 'bob@example.com',
  isRunning: 1,
});

const samples = listVmUsageSamples(testDayKey, 100);
assert(samples.length === 3, 'should record 3 samples for test day');
assert(samples[0].ownerEmail === 'alice@example.com', 'sample must record non-anonymous owner email');
assert(samples[0].vmName === "Alice's Machine", 'sample must record VM name');
assert(samples[0].isRunning === true, 'sample must record running state');
assert(samples[0].activeUsers.includes('alice@example.com'), 'active users must be parsed into array');

const timeline = getVmUsageTimeline(testDayKey);
assert(timeline.dayKey === testDayKey, 'timeline dayKey must match');
assert(timeline.hours.length === 24, 'timeline must have 24 hours');
assert(timeline.hours[2].peakRunning === 2, 'hour 2 peak running should be 2 concurrent VMs');
assert(timeline.hours[2].activeSessionsPeak === 1, 'hour 2 peak active sessions should be 1');
const hour2Users = timeline.hours[2].users;
assert(hour2Users.some(u => u.email === 'alice@example.com'), 'hour 2 must include alice (not anonymous)');
assert(hour2Users.some(u => u.email === 'bob@example.com'), 'hour 2 must include bob (not anonymous)');
assert(timeline.peakConcurrentToday === 2, 'peak concurrent today should be 2');

// Pruning test: calling pruneOldVmUsageSamples should execute without error
pruneOldVmUsageSamples(30);

// --- Inactivity Notification Policy Verification ---
const serverCode = readFileSync(join(import.meta.dir, '../server.js'), 'utf8');
assert(!serverCode.includes('setInterval(nudgeWorker,'), 'nudgeWorker interval must be removed');
assert(serverCode.includes('premiumMaintenanceWorker'), 'premium expiration maintenance worker must be preserved');
assert(serverCode.includes("Our records show you haven't logged in to mitch.pro for 5 days"), 'premium inactivity email must be preserved');

console.log('VM portal security, policy, and failure tests passed.');

