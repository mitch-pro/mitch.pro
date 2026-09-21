export function canAccessVmRecord(record, actor, options = {}) {
  if (!record || !actor) return false;
  const owner = String(record.ownerEmail || '').trim().toLowerCase();
  const email = String(actor.email || '').trim().toLowerCase();
  const isOwner = Boolean(record.status !== 'unassigned' && owner && email && owner === email);
  if (isOwner) return true;
  if (actor.isAdmin) {
    const fn = typeof options === 'function' ? options : options?.isAdminEmail;
    const ownerIsAdmin = fn
      ? (typeof fn === 'function' ? Boolean(fn(owner)) : (fn instanceof Set ? fn.has(owner) : (Array.isArray(fn) ? fn.includes(owner) : false)))
      : Boolean(record.ownerIsAdmin);
    if (owner && ownerIsAdmin) return false;
    if (options && typeof options === 'object' && options.requireAdminGrant) {
      if (typeof options.isGrantAllowed === 'function') {
        return Boolean(options.isGrantAllowed(record.id, record));
      }
      return Boolean(record.adminAccessAllowed);
    }
    return true;
  }
  return false;
}

export function validateDesktopSession(session, actor, record, now = Date.now(), options = {}) {
  if (!session || session.used || Number(session.expiresAt) <= Number(now)) return { ok: false, status: 401, code: 'expired' };
  if (!actor || !record || session.sid !== actor.sid || session.actorEmail !== actor.email) return { ok: false, status: 403, code: 'forbidden' };
  if (record.id !== session.recordId || Number(record.vmid) !== Number(session.vmid) || record.node !== session.node) return { ok: false, status: 403, code: 'forbidden' };
  if (session.ownerEmail !== undefined && session.ownerEmail !== record.ownerEmail) return { ok: false, status: 403, code: 'forbidden' };
  if (session.authSessionKey && session.authSessionKey !== actor.authSessionKey) return { ok: false, status: 403, code: 'forbidden' };
  if (!canAccessVmRecord(record, actor, options)) return { ok: false, status: 403, code: 'forbidden' };
  return { ok: true, status: 101, code: 'ok' };
}

export class VmOperationGate {
  constructor(cooldownMs = 3000) {
    this.cooldownMs = Math.max(0, Number(cooldownMs) || 0);
    this.active = new Map();
  }

  acquire(key, action, now = Date.now()) {
    const current = this.active.get(String(key));
    if (current && (current.pending || Number(now) - current.finishedAt < this.cooldownMs)) return false;
    this.active.set(String(key), { action: String(action || ''), startedAt: Number(now), pending: true });
    return true;
  }

  release(key, now = Date.now()) {
    const current = this.active.get(String(key));
    if (current) this.active.set(String(key), { ...current, pending: false, finishedAt: Number(now) });
  }

  cleanup(now = Date.now()) {
    for (const [key, state] of this.active) if (!state.pending && Number(now) - state.finishedAt >= this.cooldownMs) this.active.delete(key);
  }
}

export const VM_DEFAULT_CPU_CORES = 2;
export const VM_DEFAULT_MEMORY_MB = 4096;                    // 4 GB RAM
export const VM_DEFAULT_BALLOON_MB = 1024;                   // 1 GB minimum balloon
export const VM_DEFAULT_DISK_GB = 64;                        // 64 GB disk

export const VM_MAX_UPGRADE_CPU_CORES = 6;                   // Up to 6 cores
export const VM_MAX_UPGRADE_MEMORY_MB = 16384;               // Up to 16 GB RAM (16384 MB)
export const VM_MAX_UPGRADE_DISK_GB = 128;                   // Up to 128 GB disk

export const VM_FLEET_MAX_CORES = 36;                        // Max 36 cores total fleet
export const VM_FLEET_MAX_MEMORY_MB = 6 * 16 * 1024;         // Max 96 GB RAM (98304 MB) total fleet
export const VM_MAX_CONCURRENT_RUNNING = VM_FLEET_MAX_CORES / VM_DEFAULT_CPU_CORES; // 18 default VMs max

export const VM_DAILY_MAX_SECONDS = 6 * 3600;                // 6 hours per day = 21,600 seconds
export const VM_EXTENSION_COOLDOWN_MS = 24 * 60 * 60 * 1000; // 1 extension per 24 hours
export const VM_COOLDOWN_DURATION_MS = 30 * 60 * 1000;       // 30 minutes cooldown
export const VM_OFFPAGE_INACTIVITY_MS = 10 * 60 * 1000;      // 10 minutes off-page auto-shutdown
export const VM_ADMIN_OFFPAGE_INACTIVITY_MS = 30 * 60 * 1000; // 30 minutes off-page auto-shutdown for admins

export const VM_UPGRADE_CATALOG = {
  cpu: [
    { value: 2, label: '2 Cores (Default)', cost: 0 },
    { value: 4, label: '4 Cores', cost: 400 },
    { value: 6, label: '6 Cores (Max)', cost: 800 },
  ],
  ram: [
    { value: 4096, label: '4 GB (Default)', cost: 0 },
    { value: 8192, label: '8 GB', cost: 400 },
    { value: 12288, label: '12 GB', cost: 800 },
    { value: 16384, label: '16 GB (Max)', cost: 1200 },
  ],
  disk: [
    { value: 64, label: '64 GB (Default)', cost: 0 },
    { value: 80, label: '80 GB', cost: 150 },
    { value: 96, label: '96 GB', cost: 300 },
    { value: 112, label: '112 GB', cost: 600 },
    { value: 128, label: '128 GB (Max)', cost: 1200 },
  ],
  session: [
    { value: 6 * 3600, label: '6 Hours / Day (Default)', cost: 0, duration: 'Permanent' },
    { value: 8 * 3600, label: '8 Hours / Day (+2h) - 30-Day Pass', cost: 300, durationDays: 30 },
    { value: 10 * 3600, label: '10 Hours / Day (+4h) - 30-Day Pass', cost: 600, durationDays: 30 },
    { value: 12 * 3600, label: '12 Hours / Day (+6h) - 30-Day Pass', cost: 900, durationDays: 30 },
    { value: 24 * 3600, label: 'Unlimited (24h / Day) - 30-Day Pass', cost: 1800, durationDays: 30 },
  ],
};

export function checkFleetResourceCapacity(runningCores = 0, runningMemoryMb = 0, addingCores = 0, addingMemoryMb = 0) {
  const totalCores = Number(runningCores || 0) + Number(addingCores || 0);
  const totalMemoryMb = Number(runningMemoryMb || 0) + Number(addingMemoryMb || 0);
  const ok = totalCores <= VM_FLEET_MAX_CORES && totalMemoryMb <= VM_FLEET_MAX_MEMORY_MB;
  return {
    ok,
    totalCores,
    totalMemoryMb,
    maxCores: VM_FLEET_MAX_CORES,
    maxMemoryMb: VM_FLEET_MAX_MEMORY_MB,
  };
}

export function getRemainingDailyVmSeconds(usedSeconds, { isAdmin = false, dailyMaxSeconds = VM_DAILY_MAX_SECONDS } = {}) {
  if (isAdmin || dailyMaxSeconds >= 24 * 3600) return Infinity;
  const used = Math.max(0, Math.floor(Number(usedSeconds) || 0));
  return Math.max(0, dailyMaxSeconds - used);
}

export function isDailyVmLimitReached(usedSeconds, { isAdmin = false, dailyMaxSeconds = VM_DAILY_MAX_SECONDS } = {}) {
  if (isAdmin || dailyMaxSeconds >= 24 * 3600) return false;
  return (Number(usedSeconds) || 0) >= dailyMaxSeconds;
}

export function getVmDayKey(now = Date.now()) {
  return new Date(now).toISOString().slice(0, 10);
}

export function isEligibleForFreeVm(email, { isAdmin = false, isPremium = false } = {}) {
  if (!email) return false;
  if (isAdmin) return true;
  const e = String(email).trim().toLowerCase();
  if (e.endsWith('@student.rjuhsd.us') || e.endsWith('@student.mitch.pro')) return true;
  if (isPremium) return true;
  return false;
}

export function canUserExtend(lastExtensionAt, { isAdmin = false, now = Date.now(), cooldownMs = VM_EXTENSION_COOLDOWN_MS } = {}) {
  if (isAdmin) return true;
  if (!lastExtensionAt) return true;
  return (Number(now) - Number(lastExtensionAt)) >= cooldownMs;
}

export function computeCooldownRemaining(cooldownUntil, { isAdmin = false, now = Date.now() } = {}) {
  if (isAdmin) return 0;
  if (!cooldownUntil) return 0;
  const remMs = Number(cooldownUntil) - Number(now);
  return remMs > 0 ? Math.ceil(remMs / 1000) : 0;
}

export function isVmInactive(lastSeen, { now = Date.now(), timeoutMs, isAdmin = false } = {}) {
  if (!lastSeen) return false;
  const effectiveTimeout = timeoutMs !== undefined ? timeoutMs : (isAdmin ? VM_ADMIN_OFFPAGE_INACTIVITY_MS : VM_OFFPAGE_INACTIVITY_MS);
  return (Number(now) - Number(lastSeen)) >= effectiveTimeout;
}

export function isVmAdminAccessAllowed(recordId, grants = {}) {
  if (!recordId) return false;
  return Boolean(grants[String(recordId)]?.allowed);
}

export function isVmAdminAccessRequested(recordId, grants = {}) {
  if (!recordId) return false;
  return Boolean(grants[String(recordId)]?.requested);
}

export function shouldNotifyCapacityAlert(email, now = Date.now(), lastMap = new Map(), cooldownMs = 60_000) {
  const norm = String(email || 'unknown').trim().toLowerCase();
  const lastTime = lastMap.get(norm) || 0;
  if (now - lastTime < cooldownMs) return false;
  lastMap.set(norm, now);
  return true;
}

export function formatCapacityFullAlert(userEmail, actionDesc = 'use a computer', maxLimit = VM_MAX_CONCURRENT_RUNNING) {
  const norm = String(userEmail || 'unknown').trim().toLowerCase();
  return {
    title: 'VM Capacity Alert',
    message: `VM capacity full (${maxLimit}/${maxLimit}): ${norm} attempted to ${actionDesc}.`,
    priority: 'high',
  };
}

export function formatAdminUsageNotice(adminEmail, vmName = 'your computer', operation = 'accessed') {
  return {
    title: 'Admin Used Your Computer',
    message: `Administrator ${adminEmail} accessed your computer "${vmName}" (${operation}).`,
  };
}

export function formatAdminAccessRequest(adminEmail, vmName = 'your computer') {
  return {
    title: 'Admin Access Request',
    message: `Administrator ${adminEmail} requested access to your computer "${vmName}". You can allow or revoke access in your Computer settings.`,
  };
}

export function formatUptimeDuration(seconds) {
  const s = Math.max(0, Math.floor(Number(seconds) || 0));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}
