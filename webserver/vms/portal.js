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
  const adminPending = new Set();
  let computers = [], loading = false, provisionMode = 'create';
  let upgradeData = null, activeUpgradeTab = 'cpu', upgradeBusy = false;
  const esc = value => String(value ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const bytes = value => Number(value) ? `${(Number(value) / 1073741824).toLocaleString(undefined, { maximumFractionDigits: 1 })} GB` : '\u2014';
  const uptime = value => { const n = Number(value) || 0, d = Math.floor(n / 86400), h = Math.floor(n % 86400 / 3600), m = Math.floor(n % 3600 / 60); return !n ? '\u2014' : d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`; };
  const percent = (used, total) => total > 0 ? Math.max(0, Math.min(100, Math.round(Number(used || 0) / Number(total) * 100))) : 0;
  function infoStat(label, value, detail = '') {
    return `<div class="info-stat"><dt>${esc(label)}</dt><dd title="${esc(value)}">${esc(value)}</dd>${detail ? `<small>${esc(detail)}</small>` : ''}</div>`;
  }
  function resourceMeter(label, value, usage, active) {
    const amount = active && Number.isFinite(usage) ? `${Math.max(0, Math.min(100, usage))}%` : '—';
    return `<div class="resource-meter"><div class="resource-meta"><span><strong>${esc(label)}</strong><small>${esc(value)}</small></span><b>${amount}</b></div><div class="meter-track" ${active ? `role="meter" aria-label="${esc(label)} usage" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${usage}"` : `role="img" aria-label="${esc(label)} usage unavailable while offline"`}><span style="width:${active ? usage : 0}%"></span></div></div>`;
  }
  function setState(name) { ['loading-state', 'empty-state', 'error-state', 'computer-grid'].forEach(id => $(id).classList.toggle('is-hidden', id !== name)); }
  function card(vm) {
    const running = vm.status === 'running';
    const operation = pending.get(vm.id);
    const busy = !!operation || ['starting', 'stopping', 'restarting'].includes(vm.status);
    const status = operation || ({
      running: 'Online', stopped: 'Offline', starting: 'Starting…',
      stopping: 'Shutting down…', restarting: 'Restarting…',
      unavailable: 'Unavailable', 'setup-incomplete': 'Setup incomplete',
      unknown: 'Checking status'
    }[vm.status] || 'Offline');
    const statusTone = busy ? 'transitioning' : running ? 'running' : vm.status === 'stopped' ? 'offline' : 'unavailable';
    const distro = vm.operatingSystem || 'Linux desktop';
    const mark = /mint/i.test(distro) ? 'LM' : /ubuntu/i.test(distro) ? 'U' : 'PC';
    const open = running && !busy && vm.desktopAvailable !== false;
    const cpuLoad = Math.max(0, Math.min(100, Math.round(Number(vm.cpuUsage || 0) * 100)));
    const memoryTotal = vm.memoryTotal || (vm.upgrades?.memoryMb ? vm.upgrades.memoryMb * 1048576 : 0);
    const diskTotal = vm.diskTotal || (vm.upgrades?.diskGb ? vm.upgrades.diskGb * 1073741824 : 0);
    const memoryLoad = percent(vm.memoryUsed, memoryTotal);
    const diskLoad = percent(vm.diskUsed, diskTotal);
    const isExempt = Boolean(vm.lease?.isExempt);
    const remSeconds = vm.lease?.remainingSeconds != null ? vm.lease.remainingSeconds : null;
    const dailyUsed = Boolean(vm.lease?.dailyExtensionUsed);
    const dailyLimitReached = !isExempt && remSeconds === 0;
    const canExtend = !isExempt && running && !busy && vm.lease?.canExtend && !vm.lease?.extended && !dailyUsed;
    const inCooldown = !running && Number(vm.cooldownRemainingSeconds) > 0;
    const cooldownMins = inCooldown ? Math.ceil(Number(vm.cooldownRemainingSeconds) / 60) : 0;
    const adminAllowed = Boolean(vm.adminAccessAllowed);
    const adminRequested = Boolean(vm.adminAccessRequested);
    const changingAccess = adminPending.has(vm.id);
    const sessionValue = isExempt ? 'Unlimited' : running && remSeconds != null ? uptime(remSeconds) + ' left' : Math.round((vm.upgrades?.dailyMaxSeconds || 21600) / 3600) + 'h / day';
    const sessionDetail = vm.upgrades?.sessionUpgradeExpiresAt && vm.upgrades?.dailyMaxSeconds > 21600
      ? 'Pass: ' + Math.max(1, Math.ceil((vm.upgrades.sessionUpgradeExpiresAt - Date.now()) / 86400000)) + 'd left' : '';
    const startDisabled = busy || inCooldown || vm.status !== 'stopped' || dailyLimitReached;
    const startLabel = dailyLimitReached ? 'Daily limit reached' : inCooldown ? 'Cooldown (' + cooldownMins + 'm)' : operation || 'Start Computer';
    const previewTitle = busy ? status : vm.status === 'stopped' ? 'Computer offline' : vm.status === 'unavailable' ? 'Connection unavailable' : status;
    const previewCopy = busy ? 'This usually takes a moment.' : vm.status === 'stopped' ? 'Start your computer to connect.' : 'Refresh to check the connection.';
    const previewTag = open ? 'a' : 'div';
    const previewLink = open ? ` href="/vms/desktop/?id=${encodeURIComponent(vm.id)}" aria-label="Open ${esc(vm.name || 'My Computer')}"` : '';
    const primaryAction = running
      ? open ? `<a class="primary-button main-action" href="/vms/desktop/?id=${encodeURIComponent(vm.id)}">Open Desktop <span aria-hidden="true">↗</span></a>`
        : `<button class="primary-button main-action" disabled>${esc(busy ? status : 'Desktop unavailable')}</button>`
      : `<button class="primary-button main-action" data-action="start" ${startDisabled ? 'disabled' : ''}>${esc(startLabel)}</button>`;
    return `<article class="computer-card" data-id="${esc(vm.id)}">
      <${previewTag} class="desktop-preview ${running ? 'is-running' : 'is-offline'}"${previewLink}>
        <span class="preview-status status-pill ${statusTone}">${esc(status)}</span>
        <div class="desktop-window" aria-hidden="true">
          <div class="window-bar"><span class="window-brand">${mark}</span><span class="window-clock">${esc(vm.name || 'My Computer')}</span><span class="window-system"><i></i><i></i><i></i></span></div>
          <div class="window-content"><div class="desktop-emblem">${mark}</div><div class="desktop-dock"><i></i><i></i><i></i><i></i></div></div>
        </div>
        ${open ? '' : `<div class="preview-overlay"><strong>${esc(previewTitle)}</strong><span>${esc(previewCopy)}</span></div>`}
      </${previewTag}>
      <div class="computer-details">
        <div class="computer-title-row">
          <div><h2>${esc(vm.name || 'My Computer')}</h2><p>${esc(distro)}</p></div>
          <span class="status-pill ${statusTone}">${esc(status)}</span>
        </div>
        <section class="detail-section connection-section" aria-label="Connection">
          <h3>Connection</h3>
          <dl class="machine-facts">
            ${infoStat('IP Address', vm.ipAddress || (running ? 'Connecting…' : 'Not available'))}
            ${infoStat('Uptime', running ? uptime(vm.uptime) : '—')}
            ${infoStat(isExempt ? 'Session Limit' : running ? 'Session Time' : 'Session Limit', sessionValue, sessionDetail)}
            ${inCooldown ? infoStat('Cooldown', cooldownMins + 'm left') : ''}
          </dl>
        </section>
        <section class="detail-section resources-section" aria-labelledby="resources-${esc(vm.id)}">
          <div class="section-heading"><h3 id="resources-${esc(vm.id)}">Resources</h3><button type="button" class="text-button" data-action="open-upgrade" title="Upgrade CPU, RAM, Disk, or Session Time with MitchCoins">Edit specs <span aria-hidden="true">↗</span></button></div>
          <div class="resource-grid">
            ${resourceMeter('CPU', (vm.cpuCores || vm.upgrades?.cpuCores || '—') + ' cores', cpuLoad, running)}
            ${resourceMeter('Memory', running && memoryTotal ? bytes(vm.memoryUsed) + ' / ' + bytes(memoryTotal) : bytes(memoryTotal), memoryLoad, running && memoryTotal > 0)}
            ${resourceMeter('Storage', running && diskTotal ? bytes(vm.diskUsed) + ' / ' + bytes(diskTotal) : bytes(diskTotal), diskLoad, running && diskTotal > 0)}
          </div>
        </section>
        <section class="detail-section access-section" aria-label="Access">
          <h3>Access</h3>
          <div class="access-row"><div><strong>Admin access <span class="access-value ${adminAllowed ? 'allowed' : ''}">${adminAllowed ? 'Allowed' : 'Disabled'}</span></strong><p>Allows administrator privileges inside the desktop.</p></div><button type="button" class="quiet-button" data-action="toggle-admin-access" ${changingAccess || busy ? 'disabled' : ''}>${changingAccess ? 'Updating…' : adminAllowed ? 'Revoke' : 'Allow admin'}</button></div>
          ${adminRequested && !adminAllowed ? `<div class="admin-request-banner"><span>Administrator requested access for support.</span><button type="button" class="quiet-button" data-action="grant-admin-access" ${changingAccess || busy ? 'disabled' : ''}>Allow access</button></div>` : ''}
        </section>
        <div class="computer-actions">
          ${primaryAction}
          ${canExtend ? '<button type="button" class="control-button" data-action="extend">Extend 30m</button>' : running && dailyUsed ? '<button type="button" class="control-button" disabled title="Only one 30-minute extension is allowed per day">Extension used</button>' : ''}
          <details class="more-menu"><summary aria-label="More computer actions"><span aria-hidden="true">···</span> More</summary><div class="menu-panel" role="group" aria-label="Computer power and reset">
            ${running ? `<button type="button" data-action="restart" ${busy ? 'disabled' : ''}>Restart</button><button type="button" data-action="shutdown" ${busy ? 'disabled' : ''}>Shut Down</button><span class="menu-divider"></span>` : ''}
            <button type="button" class="danger-menu-action" data-action="recreate" ${busy ? 'disabled' : ''}>Delete &amp; Recreate Computer</button>
          </div></details>
        </div>
      </div>
    </article>`;
  }
  function render() { grid.innerHTML = computers.map(card).join(''); setState(computers.length ? 'computer-grid' : 'empty-state'); }
  async function load() {
    if (loading) return;
    loading = true;
    const refreshButton = $('refresh-button');
    refreshButton.disabled = true;
    refreshButton.classList.add('is-loading');
    refreshButton.setAttribute('aria-label', 'Refreshing computer status');
    refreshButton.querySelector('span').textContent = 'Refreshing…';
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
    } finally {
      loading = false;
      refreshButton.disabled = false;
      refreshButton.classList.remove('is-loading');
      refreshButton.setAttribute('aria-label', 'Refresh computer status');
      refreshButton.querySelector('span').textContent = 'Refresh';
    }
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
    if (!vm || adminPending.has(id) || pending.has(id)) return;
    const targetState = forceAllow ? true : !vm.adminAccessAllowed;
    adminPending.add(id);
    render();
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
      await load();
    } catch (err) {
      $('refresh-status').textContent = err.message || 'Could not update admin access.';
    } finally {
      adminPending.delete(id);
      render();
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
      if (res.status === 401) throw new Error('Please sign in to view optional VM upgrades using earned MitchCoins.');
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
    if (upgradeBusy) return;
    upgradeBusy = true;
    document.querySelectorAll('#upgrade-tab-content button[data-upgrade-cat]').forEach(button => { button.disabled = true; });
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
      }
      await load();
    } catch (err) {
      statusEl.textContent = err.message || 'Upgrade failed.';
      statusEl.style.color = '#f87171';
    } finally {
      upgradeBusy = false;
      renderUpgradeTab();
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
    button.closest('.more-menu')?.removeAttribute('open');
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
  document.addEventListener('click', event => {
    grid.querySelectorAll('.more-menu[open]').forEach(menu => { if (!menu.contains(event.target)) menu.removeAttribute('open'); });
  });
  $('refresh-button').addEventListener('click', load);
  window.addEventListener('hashchange', () => { if (location.hash === '#upgrades') openUpgradeModal(); });
  load().then(checkUrlAction);
  const timer = setInterval(() => { if (!document.hidden && !dialog.open && !provDialog?.open && !upDialog?.open && !pending.size) load(); }, 60000);
  window.addEventListener('pagehide', () => clearInterval(timer), { once: true });
})();
