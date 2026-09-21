(function () {
  'use strict';
  const $ = id => document.getElementById(id);
  const grid = $('computer-grid');
  document.title = `My Computer - ${location.hostname}`;
  const dialog = $('confirm-dialog');
  const provDialog = $('provision-dialog');
  const upDialog = $('upgrade-dialog');
  const headers = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
  const pending = new Map();
  let computers = [], loading = false, provisionMode = 'create';
  let upgradeData = null, activeUpgradeTab = 'cpu';
  const esc = value => String(value ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const bytes = value => Number(value) ? `${(Number(value) / 1073741824).toLocaleString(undefined, { maximumFractionDigits: 1 })} GB` : '\u2014';
  const uptime = value => { const n = Number(value) || 0, d = Math.floor(n / 86400), h = Math.floor(n % 86400 / 3600), m = Math.floor(n % 3600 / 60); return !n ? '\u2014' : d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`; };
  const percent = (used, total) => total > 0 ? Math.max(0, Math.min(100, Math.round(Number(used || 0) / Number(total) * 100))) : 0;
  function setState(name) { ['loading-state', 'empty-state', 'error-state', 'computer-grid'].forEach(id => $(id).classList.toggle('is-hidden', id !== name)); }
  function card(vm) {
    const running = vm.status === 'running';
    const operation = pending.get(vm.id);
    const busy = !!operation || ['starting', 'stopping', 'restarting'].includes(vm.status);
    const status = operation || ({ running: 'Running', stopped: 'Offline', starting: 'Starting...', stopping: 'Shutting down...', restarting: 'Restarting...', unavailable: 'Unavailable', unknown: 'Checking status' }[vm.status] || 'Offline');
    const distro = vm.operatingSystem || 'Linux desktop';
    const mark = /mint/i.test(distro) ? 'LM' : /ubuntu/i.test(distro) ? 'U' : 'PC';
    const open = running && !busy && vm.desktopAvailable !== false;
    const cpuLoad = Math.max(0, Math.min(100, Math.round(Number(vm.cpuUsage || 0) * 100)));
    const memoryLoad = percent(vm.memoryUsed, vm.memoryTotal);
    const diskLoad = percent(vm.diskUsed, vm.diskTotal);
    const isExempt = Boolean(vm.lease?.isExempt);
    const remSeconds = vm.lease?.remainingSeconds != null ? vm.lease.remainingSeconds : null;
    const remDisplay = remSeconds != null ? uptime(remSeconds) : null;
    const dailyUsed = Boolean(vm.lease?.dailyExtensionUsed);
    const dailyLimitReached = !isExempt && vm.lease?.remainingSeconds === 0;
    const canExtend = !isExempt && running && !busy && vm.lease?.canExtend && !vm.lease?.extended && !dailyUsed;
    const inCooldown = !running && Number(vm.cooldownRemainingSeconds) > 0;
    const cooldownMins = inCooldown ? Math.ceil(Number(vm.cooldownRemainingSeconds) / 60) : 0;
    const adminAllowed = Boolean(vm.adminAccessAllowed);
    const adminRequested = Boolean(vm.adminAccessRequested);
    const previewTag = open ? 'a' : 'div';
    const previewLink = open ? ` href="/vms/desktop/?id=${encodeURIComponent(vm.id)}" aria-label="Open ${esc(vm.name || 'My Computer')}"` : '';
    return `<article class="computer-card" data-id="${esc(vm.id)}">
      <${previewTag} class="desktop-preview ${running ? 'is-running' : 'is-offline'}"${previewLink}>
        <span class="status-pill ${busy ? 'transitioning' : running ? 'running' : ''}">${esc(status)}</span>
        <div class="desktop-window" aria-hidden="true">
          <div class="window-bar"><span class="window-brand">${mark}</span><span class="window-clock">My Computer</span><span class="window-system"><i></i><i></i><i></i></span></div>
          <div class="window-content"><div class="desktop-emblem">${mark}</div><div class="desktop-dock"><i></i><i></i><i></i><i></i></div></div>
        </div>
        ${open ? '<span class="preview-action">Open desktop <b aria-hidden="true">↗</b></span>' : ''}
      </${previewTag}>
      <div class="computer-details">
        <div class="computer-title-row"><div><p class="machine-label">Personal desktop</p><h2>${esc(vm.name || 'My Computer')}</h2><p>${esc(distro)}</p></div>${open ? `<a class="primary-button" href="/vms/desktop/?id=${encodeURIComponent(vm.id)}"><span>Open Desktop</span><b aria-hidden="true">↗</b></a>` : '<button class="primary-button" disabled>Open Desktop</button>'}</div>
        <div class="machine-facts">
          <span><small>Address</small><strong title="${esc(vm.ipAddress)}">${esc(vm.ipAddress || (running ? 'Connecting…' : 'Not available'))}</strong></span>
          <span><small>Uptime</small><strong>${uptime(vm.uptime)}</strong></span>
          ${isExempt ? `<span><small>Time Limit</small><strong style="color:#4ade80;">Unlimited</strong></span>` : (running && remDisplay ? `<span><small>Time Left</small><strong style="${remSeconds <= 600 ? 'color:#fde047' : ''}">${remDisplay}</strong></span>` : `<span><small>Session Limit</small><strong>${Math.round((vm.upgrades?.dailyMaxSeconds || 21600) / 3600)}h / day</strong>${(vm.upgrades?.sessionUpgradeExpiresAt && vm.upgrades?.dailyMaxSeconds > 21600) ? `<small style="display:block;font-size:0.68rem;color:#c084fc;">Pass: ${Math.max(1, Math.ceil((vm.upgrades.sessionUpgradeExpiresAt - Date.now()) / 86400000))}d left</small>` : ''}</span>`)}
          ${inCooldown ? `<span><small>Cooldown</small><strong style="color:#f87171;">${cooldownMins}m left</strong></span>` : ''}
          <span><small>Admin Access</small><strong style="color:${adminAllowed ? '#4ade80' : '#94a3b8'};">${adminAllowed ? 'Allowed' : 'Disallowed'}</strong></span>
        </div>
        <div class="resource-grid">
          <div class="resource"><span><small>CPU</small><b>${esc(vm.cpuCores || vm.upgrades?.cpuCores || '2')} cores</b></span><em>${cpuLoad}%</em><i><b style="width:${cpuLoad}%"></b></i></div>
          <div class="resource"><span><small>Memory</small><b>${bytes(vm.memoryTotal || (vm.upgrades?.memoryMb ? vm.upgrades.memoryMb * 1048576 : 4294967296))}</b></span><em>${memoryLoad}%</em><i><b style="width:${memoryLoad}%"></b></i></div>
          <div class="resource"><span><small>Storage</small><b>${bytes(vm.diskTotal || (vm.upgrades?.diskGb ? vm.upgrades.diskGb * 1073741824 : 68719476736))}</b></span><em>${diskLoad}%</em><i><b style="width:${diskLoad}%"></b></i></div>
        </div>
        ${adminRequested && !adminAllowed ? `<div class="admin-request-banner" style="background:rgba(234,179,8,.12);border:1px solid #eab308;border-radius:10px;padding:10px 14px;margin:14px 0 0;display:flex;align-items:center;justify-content:space-between;gap:10px;"><span style="font-size:0.85rem;color:#fde047;">⚠️ Administrator requested access to your computer for support.</span><button class="primary-button" style="min-height:32px;padding:5px 12px;font-size:0.82rem;" data-action="grant-admin-access">Allow Access</button></div>` : ''}
        <div class="computer-actions">
          ${!running ? `<button class="primary-button" data-action="start" ${busy || inCooldown || vm.status !== 'stopped' || dailyLimitReached ? 'disabled' : ''}>${dailyLimitReached ? 'Daily Limit Reached' : inCooldown ? `Cooldown (${cooldownMins}m)` : (operation || 'Start Computer')}</button>` : ''}
          <button class="control-button upgrade-control" data-action="open-upgrade" title="Upgrade CPU, RAM, Disk, or Session Time with Mitch Coins"><span aria-hidden="true">⚡</span> Upgrade Specs</button>
          ${canExtend ? `<button class="control-button" data-action="extend" ${busy ? 'disabled' : ''}><span aria-hidden="true">+</span> Extend 30m</button>` : (running && dailyUsed ? `<button class="control-button" disabled title="Only 1 30-minute extension allowed per day"><span aria-hidden="true">+</span> Extend 30m (Used)</button>` : '')}
          <button class="control-button" data-action="restart" ${!running || busy ? 'disabled' : ''}><span aria-hidden="true">↻</span> Restart</button>
          <button class="control-button danger-control" data-action="shutdown" ${!running || busy ? 'disabled' : ''}><span aria-hidden="true">⏻</span> Shut Down</button>
          <button class="control-button" data-action="toggle-admin-access" title="${adminAllowed ? 'Revoke administrator access to this computer' : 'Allow administrators to access this computer for support'}"><span aria-hidden="true">${adminAllowed ? '🔒' : '🔓'}</span> ${adminAllowed ? 'Revoke Admin' : 'Allow Admin'}</button>
          <button class="control-button danger-control" data-action="recreate" ${busy ? 'disabled' : ''} title="Delete this computer and create a fresh one"><span aria-hidden="true">⚠️</span> Delete &amp; Recreate Computer</button>
        </div>
      </div></article>`;
  }
  function render() { grid.innerHTML = computers.map(card).join(''); setState(computers.length ? 'computer-grid' : 'empty-state'); }
  async function load() {
    if (loading) return;
    loading = true; $('refresh-button').disabled = true;
    if (!computers.length) setState('loading-state');
    try {
      const response = await fetch('/api/vm/computers', { credentials: 'same-origin', cache: 'no-store' });
      if (response.status === 401) { location.href = '/enroll/?next=' + encodeURIComponent(location.pathname); return; }
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || 'Your computers could not be reached.');
      computers = Array.isArray(data.computers) ? data.computers : [];
      if (data.isEligible === false) {
        $('empty-create-box')?.classList.add('is-hidden');
        $('empty-unauthorized-box')?.classList.remove('is-hidden');
      } else {
        $('empty-create-box')?.classList.remove('is-hidden');
        $('empty-unauthorized-box')?.classList.add('is-hidden');
      }
      $('refresh-status').textContent = ''; render();
    } catch (error) {
      if (computers.length) $('refresh-status').textContent = 'Status could not be refreshed. Try again shortly.';
      else { $('error-copy').textContent = error.message; setState('error-state'); }
    } finally { loading = false; $('refresh-button').disabled = false; }
  }
  function confirmPower(action, name) {
    if (action === 'start' || action === 'extend') return Promise.resolve(true);
    if (dialog.open) return Promise.resolve(false);
    $('confirm-title').textContent = action === 'restart' ? 'Restart computer?' : 'Shut down computer?';
    $('confirm-copy').textContent = `${action === 'restart' ? 'Restart' : 'Shut down'} ${name}? Save your work inside the desktop first.`;
    $('confirm-action').textContent = action === 'restart' ? 'Restart' : 'Shut Down';
    dialog.returnValue = ''; dialog.showModal();
    return new Promise(resolve => dialog.addEventListener('close', () => resolve(dialog.returnValue === 'confirm'), { once: true }));
  }
  async function power(id, action) {
    const vm = computers.find(item => item.id === id);
    if (!vm || pending.has(id)) return;
    if (action === 'recreate') {
      openProvisionModal('recreate');
      return;
    }
    if (!await confirmPower(action, vm.name || 'your computer') || pending.has(id)) return;
    if (action === 'extend') {
      pending.set(id, 'Extending...'); render();
      try {
        const response = await fetch(`/api/vm/computers/${encodeURIComponent(id)}/extend`, { method: 'POST', credentials: 'same-origin', headers, body: '{}' });
        const data = await response.json().catch(() => ({}));
        if (!response.ok) throw new Error(data.error || 'Session could not be extended.');
        $('refresh-status').textContent = 'Session extended by 30 minutes.';
        setTimeout(() => { pending.delete(id); load(); }, 1200);
      } catch (error) { pending.delete(id); render(); $('refresh-status').textContent = error.message; }
      return;
    }
    pending.set(id, { start: 'Starting...', restart: 'Restarting...', shutdown: 'Shutting down...' }[action]); render();
    try {
      const response = await fetch(`/api/vm/computers/${encodeURIComponent(id)}/power`, { method: 'POST', credentials: 'same-origin', headers, body: JSON.stringify({ action }) });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || 'Your computer could not be reached.');
      setTimeout(() => { pending.delete(id); load(); }, 6000);
    } catch (error) { pending.delete(id); render(); $('refresh-status').textContent = error.message; }
  }

  function openProvisionModal(mode) {
    if (!provDialog) return;
    provisionMode = mode;
    const isRecreate = mode === 'recreate';
    $('provision-eyebrow').textContent = isRecreate ? 'Recreate Computer' : 'New Computer';
    $('provision-title').textContent = isRecreate ? 'Delete & Recreate Computer' : 'Set Computer Password';
    $('recreate-warning').classList.toggle('is-hidden', !isRecreate);
    $('provision-submit').textContent = isRecreate ? 'Delete & Recreate (Erase All Data)' : 'Create Computer';
    $('provision-submit').className = isRecreate ? 'danger-button' : 'primary-button';
    $('provision-password').value = '';
    $('provision-confirm-password').value = '';
    $('provision-status').textContent = '';
    provDialog.showModal();
  }

  $('provision-cancel')?.addEventListener('click', () => provDialog?.close());
  $('create-vm-btn')?.addEventListener('click', () => openProvisionModal('create'));

  $('provision-form')?.addEventListener('submit', async event => {
    event.preventDefault();
    const pass = $('provision-password').value;
    const confirm = $('provision-confirm-password').value;
    const statusEl = $('provision-status');
    if (!pass.length) {
      statusEl.textContent = 'Password cannot be empty.';
      return;
    }
    if (pass !== confirm) {
      statusEl.textContent = 'Passwords do not match.';
      return;
    }
    statusEl.textContent = provisionMode === 'recreate' ? 'Deleting old computer and provisioning new one…' : 'Provisioning your computer…';
    $('provision-submit').disabled = true;
    try {
      const endpoint = provisionMode === 'recreate' ? '/api/vm/my-computer/recreate' : '/api/vm/my-computer/create';
      const res = await fetch(endpoint, {
        method: 'POST',
        credentials: 'same-origin',
        headers,
        body: JSON.stringify({ desktopPassword: pass }),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.error || 'Failed to provision computer.');
      provDialog.close();
      $('refresh-status').textContent = 'Your computer is being prepared and will start shortly (approx 30s)...';
      setTimeout(load, 3000);
      setTimeout(load, 10000);
    } catch (err) {
      statusEl.textContent = err.message || 'An error occurred.';
    } finally {
      $('provision-submit').disabled = false;
    }
  });

  async function toggleAdminAccess(id, forceAllow = false) {
    const vm = computers.find(item => item.id === id);
    if (!vm) return;
    const targetState = forceAllow ? true : !vm.adminAccessAllowed;
    try {
      $('refresh-status').textContent = targetState ? 'Granting administrator access...' : 'Revoking administrator access...';
      const response = await fetch(`/api/vm/computers/${encodeURIComponent(id)}/admin-access`, {
        method: 'POST',
        credentials: 'same-origin',
        headers,
        body: JSON.stringify({ allow: targetState }),
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || 'Failed to update admin access.');
      $('refresh-status').textContent = data.message || (targetState ? 'Administrator access granted.' : 'Administrator access revoked.');
      load();
    } catch (err) {
      $('refresh-status').textContent = err.message || 'Could not update admin access.';
    }
  }

  function checkUrlAction() {
    try {
      const params = new URLSearchParams(location.search);
      if (params.get('action') === 'allow-admin') {
        const id = params.get('id');
        if (id) {
          history.replaceState(null, '', location.pathname);
          toggleAdminAccess(id, true);
        }
      }
      if (params.get('action') === 'upgrades' || location.hash === '#upgrades') {
        openUpgradeModal();
      }
    } catch (_) {}
  }

  async function openUpgradeModal() {
    if (!upDialog) return;
    $('upgrade-status').textContent = '';
    upDialog.showModal();
    try {
      const res = await fetch('/api/vm/upgrades', { credentials: 'same-origin', cache: 'no-store' });
      if (res.status === 401) throw new Error('Please sign in to view and purchase VM hardware upgrades.');
      if (!res.ok) throw new Error('Could not load upgrade catalog.');
      upgradeData = await res.json();
      if ($('upgrade-coin-balance')) {
        $('upgrade-coin-balance').textContent = Math.floor(upgradeData.coins || 0).toLocaleString();
      }
      renderUpgradeTab();
    } catch (err) {
      $('upgrade-status').textContent = err.message || 'Error loading upgrades.';
    }
  }

  let sessionDurationMode = 'month';

  function renderUpgradeTab() {
    const container = $('upgrade-tab-content');
    if (!container || !upgradeData) return;
    const cat = activeUpgradeTab;
    const tiers = upgradeData.catalog?.[cat] || [];
    const current = upgradeData.current || {};
    const coins = upgradeData.coins || 0;

    let currentVal = 0;
    if (cat === 'cpu') currentVal = current.cpuCores || 2;
    if (cat === 'ram') currentVal = current.memoryMb || 4096;
    if (cat === 'disk') currentVal = current.diskGb || 64;
    if (cat === 'session') currentVal = current.dailyMaxSeconds || 21600;

    const currentTier = tiers.find(t => t.value === currentVal) || { cost: 0 };
    const expiresAt = current.sessionUpgradeExpiresAt;
    const daysLeft = (expiresAt && expiresAt > Date.now()) ? Math.ceil((expiresAt - Date.now()) / 86400000) : 0;
    const sessionMult = (cat === 'session' && sessionDurationMode === 'week') ? 0.35 : 1.0;

    let durationSelectorHtml = '';
    if (cat === 'session') {
      durationSelectorHtml = `
        <div style="display:flex;align-items:center;justify-content:space-between;background:rgba(255,255,255,0.04);border:1px solid rgba(255,255,255,0.08);border-radius:8px;padding:8px 12px;margin-bottom:12px;flex-wrap:wrap;gap:8px;">
          <div style="font-size:0.84rem;color:#cbd5e1;">
            <strong>Pass Term:</strong>
            ${daysLeft > 0 ? `<span style="margin-left:8px;color:#c084fc;font-weight:600;">Active pass expires in ${daysLeft}d</span>` : '<span style="margin-left:8px;color:#94a3b8;">Select 1 Week or 1 Month</span>'}
          </div>
          <div style="display:flex;gap:6px;">
            <button type="button" class="${sessionDurationMode === 'month' ? 'primary-button' : 'quiet-button'}" data-term="month" style="min-height:30px;padding:4px 10px;font-size:0.8rem;">1 Month (30d)</button>
            <button type="button" class="${sessionDurationMode === 'week' ? 'primary-button' : 'quiet-button'}" data-term="week" style="min-height:30px;padding:4px 10px;font-size:0.8rem;">1 Week (7d)</button>
          </div>
        </div>
      `;
    }

    container.innerHTML = `
      ${durationSelectorHtml}
      <div class="upgrade-tier-list">
      ${tiers.map(tier => {
        const isCurrent = tier.value === currentVal;
        const isOwned = tier.value <= currentVal;
        const tierCost = (tier.cost === 0) ? 0 : Math.max(1, Math.round(tier.cost * sessionMult));
        const currentTierCost = (currentTier.cost === 0) ? 0 : Math.max(1, Math.round(currentTier.cost * sessionMult));
        const diffCost = (isCurrent && cat === 'session' && currentVal > 21600)
          ? tierCost
          : Math.max(0, tierCost - currentTierCost);
        const canAfford = coins >= diffCost;

        let actionHtml = '';
        if (isCurrent && cat === 'session' && currentVal > 21600) {
          actionHtml = `<span class="current-tier-pill" style="margin-bottom:4px;">✓ Active (${daysLeft}d left)</span><button type="button" class="quiet-button" data-upgrade-cat="${esc(cat)}" data-upgrade-val="${tier.value}" data-duration="${sessionDurationMode}" style="min-height:30px;padding:4px 10px;font-size:0.8rem;">Renew (+${sessionDurationMode === 'week' ? '7d' : '30d'} for ${diffCost} 🪙)</button>`;
        } else if (isCurrent) {
          actionHtml = `<span class="current-tier-pill">✓ Current</span>`;
        } else if (isOwned && cat !== 'session') {
          actionHtml = `<span class="current-tier-pill" style="opacity:0.75;">Included</span>`;
        } else if (canAfford) {
          actionHtml = `<button type="button" class="primary-button" data-upgrade-cat="${esc(cat)}" data-upgrade-val="${tier.value}" data-duration="${sessionDurationMode}" style="min-height:36px;padding:6px 14px;font-size:0.85rem;">Upgrade for ${diffCost} 🪙</button>`;
        } else {
          actionHtml = `<button type="button" class="quiet-button" disabled style="font-size:0.82rem;padding:6px 12px;">Need ${diffCost} 🪙</button>`;
        }

        const subLabel = (cat === 'session' && tier.cost > 0)
          ? `Pass Cost: ${tierCost} coins (${sessionDurationMode === 'week' ? '7 Days' : '30 Days'})`
          : (tier.cost === 0 ? 'Free (Standard tier)' : `Base Tier: ${tier.cost} coins`);

        return `<div class="upgrade-tier-card ${isCurrent ? 'is-current' : ''}">
          <div class="upgrade-tier-info">
            <span class="upgrade-tier-title">${esc(tier.label)}</span>
            <span class="upgrade-tier-sub">${subLabel}</span>
          </div>
          <div class="upgrade-tier-action" style="display:flex;flex-direction:column;align-items:flex-end;">
            ${actionHtml}
          </div>
        </div>`;
      }).join('')}
    </div>`;

    document.querySelectorAll('.upgrade-tab').forEach(tab => {
      tab.classList.toggle('active', tab.dataset.tab === cat);
    });
  }

  async function purchaseUpgrade(category, targetValue, duration) {
    const statusEl = $('upgrade-status');
    statusEl.textContent = 'Applying upgrade…';
    statusEl.style.color = '#fde047';
    try {
      const res = await fetch('/api/vm/upgrade', {
        method: 'POST',
        credentials: 'same-origin',
        headers,
        body: JSON.stringify({ category, targetValue, duration: duration || (category === 'session' ? sessionDurationMode : undefined) }),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.error || 'Failed to apply upgrade.');
      statusEl.textContent = data.message || 'Upgrade successful!';
      statusEl.style.color = '#4ade80';
      if (upgradeData) {
        upgradeData.current = data.upgrades;
        upgradeData.coins = data.coins;
        if ($('upgrade-coin-balance')) $('upgrade-coin-balance').textContent = Math.floor(data.coins).toLocaleString();
        renderUpgradeTab();
      }
      load();
    } catch (err) {
      statusEl.textContent = err.message || 'Upgrade failed.';
      statusEl.style.color = '#f87171';
    }
  }

  $('upgrade-close-btn')?.addEventListener('click', () => upDialog?.close());

  document.querySelectorAll('.upgrade-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      activeUpgradeTab = tab.dataset.tab;
      renderUpgradeTab();
    });
  });

  $('upgrade-tab-content')?.addEventListener('click', event => {
    const termBtn = event.target.closest('button[data-term]');
    if (termBtn) {
      sessionDurationMode = termBtn.dataset.term;
      renderUpgradeTab();
      return;
    }
    const btn = event.target.closest('button[data-upgrade-cat]');
    if (!btn || btn.disabled) return;
    const cat = btn.dataset.upgradeCat;
    const val = Number(btn.dataset.upgradeVal);
    const dur = btn.dataset.duration;
    if (cat && Number.isFinite(val)) {
      purchaseUpgrade(cat, val, dur);
    }
  });

  grid.addEventListener('click', event => {
    const button = event.target.closest('button[data-action]');
    if (!button || button.disabled) return;
    const action = button.dataset.action;
    const cardEl = button.closest('[data-id]');
    const id = cardEl?.dataset.id;
    if (!id) return;
    if (action === 'open-upgrade') {
      openUpgradeModal();
      return;
    }
    if (action === 'grant-admin-access') {
      toggleAdminAccess(id, true);
      return;
    }
    if (action === 'toggle-admin-access') {
      toggleAdminAccess(id);
      return;
    }
    power(id, action);
  });
  $('refresh-button').addEventListener('click', load);
  window.addEventListener('hashchange', () => { if (location.hash === '#upgrades') openUpgradeModal(); });
  load().then(checkUrlAction);
  const timer = setInterval(() => { if (!document.hidden && !dialog.open && !provDialog?.open && !upDialog?.open && !pending.size) load(); }, 60000);
  window.addEventListener('pagehide', () => clearInterval(timer), { once: true });
})();
