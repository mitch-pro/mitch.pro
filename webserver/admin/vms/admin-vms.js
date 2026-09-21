(function () {
  'use strict';
  const headers = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
  const $ = id => document.getElementById(id);
  const esc = value => String(value ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const bytes = value => Number(value) ? `${(Number(value) / 1073741824).toLocaleString(undefined, { maximumFractionDigits: 1 })} GB` : '\u2014';
  const pct = (used, total) => total ? Math.max(0, Math.min(100, Math.round(used / total * 100))) : 0;
  const pending = new Set(), failedDeletes = new Set();
  let overview = null, loading = false, creating = false, assigning = false;
  function getAdminHeaders(extra = {}) {
    const h = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1', ...extra };
    const pass = sessionStorage.getItem('admin_passphrase');
    if (pass) h['X-Admin-Passphrase'] = pass;
    return h;
  }

  let passphraseResolvers = [];
  function showPassphraseLockBox(isNewSetup = false) {
    const box = $('passphrase-lock-box');
    if (!box) return;
    box.style.display = 'block';
    $('lock-box-headline').textContent = isNewSetup ? 'Set Admin Passphrase' : 'Passphrase Verification Required';
    $('lock-box-desc').textContent = isNewSetup
      ? 'Create a secure passphrase to protect all administrative actions.'
      : 'Administrative endpoints are protected. Enter your admin passphrase to unlock computer management.';
    $('admin-passphrase-input').placeholder = isNewSetup ? 'Choose secure passphrase...' : 'Admin passphrase...';
    $('admin-passphrase-btn').textContent = isNewSetup ? 'Set Passphrase' : 'Unlock';
    $('admin-passphrase-input').value = '';
    $('admin-passphrase-input').focus();
    $('passphrase-status').textContent = '';
    $('passphrase-status').className = 'form-status';
  }

  function hidePassphraseLockBox() {
    const box = $('passphrase-lock-box');
    if (box) box.style.display = 'none';
  }

  function waitForPassphrase(isNewSetup = false) {
    showPassphraseLockBox(isNewSetup);
    return new Promise((resolve, reject) => {
      passphraseResolvers.push({ resolve, reject });
    });
  }

  async function api(url, body, allowPrompt = true) {
    const response = await fetch(url, {
      credentials: 'same-origin',
      cache: 'no-store',
      headers: getAdminHeaders(),
      ...(body ? { method: 'POST', body: JSON.stringify(body) } : {})
    });
    const data = await response.json().catch(() => ({}));
    if (response.status === 401) {
      location.href = '/enroll/?next=' + encodeURIComponent(location.pathname);
      throw new Error('Sign in to continue.');
    }
    if (response.status === 403 && (data.error === 'invalid_passphrase' || data.error === 'passphrase_not_configured')) {
      if (allowPrompt) {
        sessionStorage.removeItem('admin_passphrase');
        await waitForPassphrase(data.error === 'passphrase_not_configured');
        return api(url, body, false);
      }
      throw new Error('Admin passphrase verification required.');
    }
    if (!response.ok) throw new Error(response.status === 403 ? 'Administrator access is required.' : data.message || data.error || 'The computer service could not be reached.');
    return data;
  }
  function options(id, rows, value, label, empty) {
    const select = $(id), previous = select.value;
    select.innerHTML = rows.map(row => `<option value="${esc(value(row))}">${esc(label(row))}</option>`).join('') || `<option value="">${empty}</option>`;
    if ([...select.options].some(option => option.value === previous)) select.value = previous;
  }
  function renderCapacity() {
    const c = overview.capacity || {};
    $('cpu-capacity').textContent = c.cpuCores ? `${pct(c.cpuUsage, 1)}% of ${c.cpuCores} cores` : '\u2014';
    $('cpu-bar').style.width = `${pct(c.cpuUsage, 1)}%`;
    $('memory-capacity').textContent = c.memoryTotal ? `${bytes(c.memoryUsed)} / ${bytes(c.memoryTotal)}` : '\u2014';
    $('memory-bar').style.width = `${pct(c.memoryUsed, c.memoryTotal)}%`;
    $('storage-capacity').textContent = c.storageTotal ? `${bytes(c.storageUsed)} / ${bytes(c.storageTotal)}` : '\u2014';
    $('storage-bar').style.width = `${pct(c.storageUsed, c.storageTotal)}%`;
    $('assigned-capacity').textContent = (overview.computers || []).filter(vm => vm.assignmentStatus !== 'unassigned').length;
    const active = (overview.activeSessions || []).length;
    if ($('active-sessions-count')) $('active-sessions-count').textContent = active;
    if ($('active-sessions-sub')) $('active-sessions-sub').textContent = `${active} user${active === 1 ? '' : 's'} on desktop`;
    const runningCores = overview.runningNonAdminCores != null ? overview.runningNonAdminCores : (overview.runningNonAdminCount ? overview.runningNonAdminCount * 2 : 0);
    const maxCores = overview.maxFleetCores || 36;
    const runningMemGb = overview.runningNonAdminMemoryMb != null ? Math.round(overview.runningNonAdminMemoryMb / 1024) : 0;
    const maxMemGb = overview.maxFleetMemoryMb != null ? Math.round(overview.maxFleetMemoryMb / 1024) : 96;
    const runningCount = overview.runningNonAdminCount != null ? overview.runningNonAdminCount : 0;
    if ($('running-vms-count')) $('running-vms-count').textContent = `${runningCores}c / ${runningMemGb}GB`;
    if ($('running-vms-sub')) $('running-vms-sub').textContent = `${runningCount} VMs (${maxCores}c, ${maxMemGb}GB max)`;
  }
  function renderForms() {
    for (const id of ['create-user', 'assign-user']) options(id, overview.users || [], user => user.email, user => `${user.name || user.email} - ${user.email}`, 'No users found');
    options('create-template', overview.templates || [], vm => vm.vmid, vm => vm.name, 'No desktop template available');
    options('assign-vm', overview.availableGuests || [], vm => vm.vmid, vm => `${vm.name} - ${vm.status}`, 'No unassigned computers');
    if (!$('create-hostname').value && $('create-user').value) setHostname();
    $('create-button').disabled = creating || !overview.serviceAvailable || !$('create-user').value || !$('create-template').value;
    $('assign-button').disabled = assigning || !overview.serviceAvailable || !$('assign-vm').value || !$('assign-user').value;
  }
  function renderFleet() {
    const rows = (overview.computers || []).filter(vm => vm.assignmentStatus !== 'unassigned');
    $('fleet-list').innerHTML = rows.length ? rows.map(vm => {
      const running = vm.status === 'running', busy = pending.has(vm.id), stopped = vm.status === 'stopped';
      const hasFailedDelete = failedDeletes.has(vm.id);
      const inUse = Array.isArray(vm.activeUsers) && vm.activeUsers.length > 0;
      const activeUserLabel = inUse ? vm.activeUsers.map(u => u.actorEmail).join(', ') : '';
      const canAccess = vm.canAccess !== false;
      const openDesktopAction = !canAccess
        ? `<span class="fleet-admin-protected" style="display:inline-flex; align-items:center; gap:4px; font-size:0.8rem; color:#94a3b8; font-weight:600; padding:4px 8px; background:rgba(255,255,255,0.04); border-radius:6px; border:1px solid rgba(255,255,255,0.08);" title="Admins cannot access other admins' computers">🔒 Admin Protected</span>`
        : (running && vm.desktopAvailable !== false ? `<a href="/vms/desktop/?id=${encodeURIComponent(vm.id)}">Open Desktop</a>` : `<button data-power="start" ${busy || !stopped ? 'disabled' : ''}>Start</button>`);
      const powerButtons = canAccess
        ? `<button data-power="restart" ${busy || !running ? 'disabled' : ''}>Restart</button><button data-power="shutdown" ${busy || !running ? 'disabled' : ''}>Shut Down</button><button data-power="force-stop" class="danger" ${busy || !running ? 'disabled' : ''}>Force Stop</button>`
        : '';
      return `<article class="fleet-item" data-id="${esc(vm.id)}"><div class="fleet-identity"><strong>${esc(vm.name)}</strong><small>${esc(vm.operatingSystem || 'Linux desktop')}</small></div><div class="fleet-owner"><strong title="${esc(vm.ownerEmail)}">${esc(vm.ownerEmail)}</strong><small>${esc(vm.hostname || 'No hostname')}</small></div><span class="fleet-state ${running ? 'running' : ''}">${busy ? 'Updating...' : running ? 'Running' : stopped ? 'Offline' : esc(vm.status)}</span>${inUse ? `<span class="fleet-in-use" style="background:#15803d; color:#f0fdf4; font-size:0.75rem; font-weight:700; padding:2px 8px; border-radius:999px; margin-left:4px;" title="Active desktop session">👤 In Use (${esc(activeUserLabel)})</span>` : ''}<span class="fleet-resources">${esc(vm.cpuCores)} CPU &middot; ${bytes(vm.memoryTotal)}<small>${bytes(vm.diskTotal)} disk</small></span><span class="fleet-address">${esc(vm.ipAddress || 'No IP yet')}</span><div class="fleet-actions">${openDesktopAction}${powerButtons}<button data-unassign class="unassign" ${busy ? 'disabled' : ''}>Unassign</button><button data-delete class="danger" ${busy ? 'disabled' : ''}>Delete</button>${hasFailedDelete ? `<button data-force-delete class="danger" style="background:#ef4444; color:#fff; border-color:#ef4444; font-weight:700;" ${busy ? 'disabled' : ''}>⚠️ Force Delete</button>` : ''}</div></article>`;
    }).join('') : '<p class="empty">No customer computers are assigned.</p>';
    if (overview.viewerIsOwner) {
      rows.forEach(vm => {
        const item = $('fleet-list').querySelector(`[data-id="${CSS.escape(vm.id)}"]`);
        if (!item) return;
        const box = document.createElement('div');
        box.className = 'fleet-credentials' + (vm.desktopCredentials ? '' : ' unavailable');
        const label = document.createElement('small');
        label.textContent = 'Desktop login';
        box.appendChild(label);
        const makeResetButton = usernameHint => {
          const reset = document.createElement('button');
          reset.type = 'button';
          reset.textContent = 'Set new login';
          reset.addEventListener('click', async event => {
            event.stopPropagation();
            const username = prompt('Desktop username', usernameHint || 'desktop');
            if (!username) return;
            const password = prompt('New desktop password (any non-empty length)');
            if (!password) return;
            reset.disabled = true;
            reset.textContent = 'Updating...';
            try {
              await api('/api/admin/vms/credentials', { id: vm.id, username, password });
              status('fleet-status', 'Desktop login updated and saved securely for owners.', 'success');
              await load();
            } catch (error) {
              status('fleet-status', error.message, 'error');
              reset.disabled = false;
              reset.textContent = 'Set new login';
            }
          });
          return reset;
        };
        if (vm.desktopCredentials) {
          const username = document.createElement('code');
          username.textContent = vm.desktopCredentials.username || 'desktop';
          const password = document.createElement('code');
          password.textContent = vm.desktopCredentials.password;
          const copy = document.createElement('button');
          copy.type = 'button';
          copy.textContent = 'Copy password';
          copy.addEventListener('click', async event => {
            event.stopPropagation();
            await navigator.clipboard.writeText(vm.desktopCredentials.password);
            copy.textContent = 'Copied';
            setTimeout(() => { copy.textContent = 'Copy password'; }, 1200);
          });
          box.append(username, password, copy, makeResetButton(vm.desktopCredentials.username));
        } else {
          const unavailable = document.createElement('span');
          unavailable.textContent = 'Not stored for this older VM. Start it, then set a new login here.';
          box.append(unavailable, makeResetButton('desktop'));
        }
        item.insertBefore(box, item.querySelector('.fleet-actions'));
      });
    }
  }
  function renderAudit() {
    const rows = overview.audit || [];
    $('audit-list').innerHTML = rows.length ? rows.map(row => `<div class="audit-row"><time>${esc(new Date(row.ts).toLocaleString('en-US', { dateStyle: 'short', timeStyle: 'short' }))}</time><strong>${esc(String(row.action || '').replaceAll('_', ' '))}</strong><span>${esc(row.actorEmail)}${row.ownerEmail ? ` &rarr; ${esc(row.ownerEmail)}` : ''}</span><span class="${row.success ? '' : 'failed'}">${row.success ? 'Success' : 'Failed'}</span></div>`).join('') : '<p class="empty">No activity yet.</p>';
  }

  let activeHoverHour = -1;

  function renderUsageStats() {
    const stats = overview?.usageStats || {};
    const summary = stats.summary || {};
    const timeline = stats.hourlyTimeline || [];
    const rankings = stats.rankings || [];

    if ($('top-user-display')) {
      if (summary.topUser && summary.topUser.todaySeconds > 0) {
        $('top-user-display').textContent = `${summary.topUser.displayName || summary.topUser.email} (${summary.topUser.todayFormatted})`;
        $('top-user-display').title = summary.topUser.email;
      } else if (summary.topUser) {
        $('top-user-display').textContent = `${summary.topUser.displayName || summary.topUser.email} (0m)`;
      } else {
        $('top-user-display').textContent = 'None yet';
      }
    }
    if ($('fleet-total-hours')) {
      $('fleet-total-hours').textContent = `${summary.totalFleetHoursToday || '0.0'} hrs`;
    }
    if ($('fleet-peak-concurrent')) {
      $('fleet-peak-concurrent').textContent = `${summary.peakRunningToday || 0} / 6`;
    }

    drawUsageChart(timeline);

    const tbody = $('leaderboard-body');
    if (!tbody) return;

    if (!rankings.length) {
      tbody.innerHTML = '<tr><td colspan="6" class="empty">No computer usage recorded yet today.</td></tr>';
      return;
    }

    tbody.innerHTML = rankings.map(u => {
      const rankClass = u.rank === 1 ? 'rank-1' : u.rank === 2 ? 'rank-2' : u.rank === 3 ? 'rank-3' : '';
      const rankEmoji = u.rank === 1 ? '🥇' : u.rank === 2 ? '🥈' : u.rank === 3 ? '🥉' : `#${u.rank}`;
      const pctOfDay = Math.min(100, Math.round((u.todaySeconds / (6 * 3600)) * 100));
      const statusBadge = u.isInUse
        ? `<span class="badge" style="background:#15803d;color:#f0fdf4;font-weight:700;">👤 In Use</span>`
        : u.isRunning
          ? `<span class="badge" style="background:#0369a1;color:#f0f9ff;font-weight:700;">🟢 Running</span>`
          : `<span class="badge" style="background:var(--soft);color:var(--muted);">Offline</span>`;

      return `<tr>
        <td><span class="rank-badge ${rankClass}">${rankEmoji}</span></td>
        <td><strong>${esc(u.displayName)}</strong><small title="${esc(u.email)}">${esc(u.email)}</small></td>
        <td><strong>${esc(u.vmName)}</strong>${u.vmid ? `<small>VMID ${u.vmid}</small>` : ''}</td>
        <td>
          <div class="uptime-bar-wrap">
            <span>${esc(u.todayFormatted)}</span>
            <div class="uptime-bar" title="${pctOfDay}% of 6h daily limit"><i style="width:${pctOfDay}%"></i></div>
          </div>
        </td>
        <td><span style="color:var(--muted);font-variant-numeric:tabular-nums;">${esc(u.allTimeFormatted)}</span></td>
        <td>${statusBadge}</td>
      </tr>`;
    }).join('');
  }

  function drawUsageChart(timeline) {
    const canvas = $('usage-chart');
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    if (rect.width <= 0) return;

    canvas.width = rect.width * dpr;
    canvas.height = (rect.height || 180) * dpr;
    ctx.scale(dpr, dpr);

    const w = rect.width;
    const h = rect.height || 180;
    const padding = { top: 20, right: 20, bottom: 30, left: 35 };
    const chartW = w - padding.left - padding.right;
    const chartH = h - padding.top - padding.bottom;

    ctx.clearRect(0, 0, w, h);

    let maxVal = 6;
    for (const item of (timeline || [])) {
      if (item.peakRunning > maxVal) maxVal = item.peakRunning;
    }
    maxVal = Math.max(maxVal, 6);

    ctx.strokeStyle = 'rgba(255, 255, 255, 0.08)';
    ctx.lineWidth = 1;
    ctx.fillStyle = '#64748b';
    ctx.font = '10px Inter, ui-sans-serif, system-ui, sans-serif';
    ctx.textAlign = 'right';

    const ySteps = 3;
    for (let i = 0; i <= ySteps; i++) {
      const val = Math.round((maxVal / ySteps) * i);
      const y = padding.top + chartH - (val / maxVal) * chartH;
      ctx.beginPath();
      ctx.moveTo(padding.left, y);
      ctx.lineTo(w - padding.right, y);
      ctx.stroke();
      ctx.fillText(String(val), padding.left - 8, y + 3);
    }

    const colW = chartW / 24;

    (timeline || []).forEach((slot, i) => {
      const x = padding.left + i * colW;
      const isHovered = (activeHoverHour === i);

      if (isHovered) {
        ctx.fillStyle = 'rgba(255, 255, 255, 0.07)';
        ctx.fillRect(x, padding.top, colW, chartH);
      }

      const runVal = Math.min(slot.peakRunning || 0, maxVal);
      const runBarH = (runVal / maxVal) * chartH;
      const runY = padding.top + chartH - runBarH;

      const actVal = Math.min(slot.activeSessionsPeak || 0, maxVal);
      const actBarH = (actVal / maxVal) * chartH;
      const actY = padding.top + chartH - actBarH;

      if (runVal > 0) {
        ctx.fillStyle = isHovered ? '#7dd3fc' : '#38bdf8';
        const barWidth = Math.max(4, colW - 4);
        const barX = x + 2;
        ctx.beginPath();
        if (ctx.roundRect) ctx.roundRect(barX, runY, barWidth, runBarH, [3, 3, 0, 0]);
        else ctx.rect(barX, runY, barWidth, runBarH);
        ctx.fill();
      }

      if (actVal > 0) {
        ctx.fillStyle = '#4ade80';
        const actWidth = Math.max(2, (colW - 4) * 0.5);
        const actX = x + 2 + ((colW - 4) - actWidth) / 2;
        ctx.beginPath();
        if (ctx.roundRect) ctx.roundRect(actX, actY, actWidth, actBarH, [2, 2, 0, 0]);
        else ctx.rect(actX, actY, actWidth, actBarH);
        ctx.fill();
      }

      if (i % 3 === 0 || i === 23) {
        ctx.fillStyle = isHovered ? '#f1f5f9' : '#64748b';
        ctx.textAlign = 'center';
        ctx.fillText(slot.label, x + colW / 2, h - 10);
      }
    });

    ctx.strokeStyle = 'rgba(255, 255, 255, 0.16)';
    ctx.beginPath();
    ctx.moveTo(padding.left, padding.top + chartH);
    ctx.lineTo(w - padding.right, padding.top + chartH);
    ctx.stroke();
  }

  function setupChartInteraction() {
    const canvas = $('usage-chart');
    const tooltip = $('chart-tooltip');
    if (!canvas || !tooltip) return;

    canvas.addEventListener('mousemove', e => {
      const timeline = overview?.usageStats?.hourlyTimeline;
      if (!timeline) return;
      const rect = canvas.getBoundingClientRect();
      const paddingLeft = 35;
      const paddingRight = 20;
      const chartW = rect.width - paddingLeft - paddingRight;
      const mouseX = e.clientX - rect.left - paddingLeft;

      if (mouseX < 0 || mouseX > chartW) {
        activeHoverHour = -1;
        tooltip.style.display = 'none';
        drawUsageChart(timeline);
        return;
      }

      const hour = Math.floor((mouseX / chartW) * 24);
      if (hour < 0 || hour >= 24) return;

      activeHoverHour = hour;
      drawUsageChart(timeline);

      const slot = timeline[hour];
      if (!slot) return;

      let userLines = '';
      if (slot.users && slot.users.length) {
        userLines = '<div style="margin-top:6px;border-top:1px solid rgba(255,255,255,.12);padding-top:5px;">' +
          slot.users.map(u => `<span class="user-item">👤 <strong>${esc(u.email)}</strong> (${esc(u.vmName || 'Computer')})</span>`).join('') +
          '</div>';
      } else {
        userLines = '<div style="margin-top:4px;color:#94a3b8;font-size:0.72rem;">No running computers</div>';
      }

      tooltip.innerHTML = `<strong>${esc(slot.fullLabel)} – ${esc(slot.label)}</strong>` +
        `<div>Running: <strong style="color:#38bdf8;">${slot.peakRunning || 0}</strong> &middot; Sessions: <strong style="color:#4ade80;">${slot.activeSessionsPeak || 0}</strong></div>` +
        userLines;

      tooltip.style.display = 'block';
      const tipX = Math.min(e.clientX - rect.left + 15, rect.width - 260);
      const tipY = Math.max(10, e.clientY - rect.top - 40);
      tooltip.style.left = `${Math.max(10, tipX)}px`;
      tooltip.style.top = `${tipY}px`;
    });

    canvas.addEventListener('mouseleave', () => {
      activeHoverHour = -1;
      tooltip.style.display = 'none';
      if (overview?.usageStats?.hourlyTimeline) {
        drawUsageChart(overview.usageStats.hourlyTimeline);
      }
    });

    window.addEventListener('resize', () => {
      if (overview?.usageStats?.hourlyTimeline) {
        drawUsageChart(overview.usageStats.hourlyTimeline);
      }
    });
  }

  async function load() {
    if (loading) return;
    loading = true; $('refresh-button').disabled = true;
    try {
      overview = await api('/api/admin/vms/overview');
      $('service-state').className = `service-state ${overview.serviceAvailable ? 'online' : 'offline'}`;
      $('service-state').querySelector('span').textContent = overview.serviceAvailable ? 'Computer service online' : 'Computer service unavailable';
      renderCapacity(); renderForms(); renderFleet(); renderAudit(); renderUsageStats();
    } catch (error) { $('service-state').className = 'service-state offline'; $('service-state').querySelector('span').textContent = error.message; }
    finally { loading = false; $('refresh-button').disabled = false; }
  }
  function status(id, message, type = '') { $(id).textContent = message; $(id).className = `form-status ${type}`; }
  function setHostname() { $('create-hostname').value = ('computer-' + $('create-user').value.split('@')[0]).toLowerCase().replace(/[^a-z0-9-]/g, '-').slice(0, 48).replace(/-+$/, ''); }
  $('create-user').addEventListener('change', setHostname);
  const passForm = $('passphrase-form');
  if (passForm) {
    passForm.addEventListener('submit', async event => {
      event.preventDefault();
      const input = $('admin-passphrase-input');
      const pass = input.value.trim();
      if (!pass) return;
      const btn = $('admin-passphrase-btn');
      btn.disabled = true;
      status('passphrase-status', 'Verifying passphrase...');
      try {
        const verifyRes = await fetch('/api/admin/passphrase-status', {
          method: 'POST',
          credentials: 'same-origin',
          headers: getAdminHeaders({ 'X-Admin-Passphrase': pass }),
          body: JSON.stringify({ passphrase: pass })
        });
        const d = await verifyRes.json().catch(() => ({}));
        if (verifyRes.ok && d.ok) {
          sessionStorage.setItem('admin_passphrase', pass);
          status('passphrase-status', 'Passphrase verified.', 'success');
          setTimeout(() => {
            hidePassphraseLockBox();
            const queue = passphraseResolvers;
            passphraseResolvers = [];
            queue.forEach(p => p.resolve(pass));
            load();
          }, 300);
        } else {
          sessionStorage.removeItem('admin_passphrase');
          status('passphrase-status', d.error === 'passphrase_too_short' ? 'Passphrase must be at least 4 characters.' : 'Incorrect admin passphrase. Try again.', 'error');
          input.select();
        }
      } catch (err) {
        status('passphrase-status', err.message || 'Verification failed.', 'error');
      } finally {
        btn.disabled = false;
      }
    });
  }
  $('create-form').addEventListener('submit', async event => {
    event.preventDefault(); if (creating) return;
    if (!confirm('Create and start this desktop? Make sure you have securely saved the desktop login details for the user.')) return;
    creating = true; const button = $('create-button'); button.disabled = true; button.textContent = 'Creating computer...';
    status('create-status', 'Cloning the desktop. Keep this page open; this can take a few minutes.');
    const body = { ownerEmail: $('create-user').value, friendlyName: $('create-name').value.trim(), hostname: $('create-hostname').value.trim(), templateVmid: Number($('create-template').value), cpuCores: Number($('create-cpu').value), memoryMb: Number($('create-memory').value), diskGb: Number($('create-disk').value), desktopUsername: $('create-username').value.trim(), desktopPassword: $('create-password').value };
    $('create-password').value = '';
    try { await api('/api/admin/vms/create', body); status('create-status', 'Computer created. The desktop may take a minute to finish starting.', 'success'); await load(); }
    catch (error) { status('create-status', error.message + ' Re-enter the desktop password before trying again.', 'error'); }
    finally { body.desktopPassword = ''; creating = false; button.textContent = 'Create & start computer'; if (overview) renderForms(); }
  });
  $('assign-form').addEventListener('submit', async event => {
    event.preventDefault(); if (assigning) return;
    assigning = true; $('assign-button').disabled = true; status('assign-status', 'Assigning...');
    try { await api('/api/admin/vms/assign', { vmid: Number($('assign-vm').value), ownerEmail: $('assign-user').value, friendlyName: $('assign-name').value.trim(), operatingSystem: $('assign-os').value }); status('assign-status', 'Computer assigned.', 'success'); await load(); }
    catch (error) { status('assign-status', error.message, 'error'); }
    finally { assigning = false; if (overview) renderForms(); }
  });
  $('fleet-list').addEventListener('click', async event => {
    const button = event.target.closest('button'), item = button?.closest('[data-id]');
    if (!item || button.disabled || pending.has(item.dataset.id)) return;
    const vm = overview?.computers.find(row => row.id === item.dataset.id); if (!vm) return;
    let url, body;
    if (button.hasAttribute('data-delete')) {
      if (!confirm(`Permanently delete computer "${vm.name}" (${vm.vmid})? This will destroy the VM on Proxmox and remove it from the system.`)) return;
      url = '/api/admin/vms/delete'; body = { id: vm.id, force: false };
    } else if (button.hasAttribute('data-force-delete')) {
      if (!confirm(`Force delete computer "${vm.name}" (${vm.vmid})? This will ignore any Proxmox errors and remove it from the system anyway.`)) return;
      url = '/api/admin/vms/delete'; body = { id: vm.id, force: true };
    } else if (button.hasAttribute('data-unassign')) {
      if (!confirm(`Unassign ${vm.name} from ${vm.ownerEmail}? Their open desktop will disconnect. The computer and its files will remain on the server.`)) return;
      url = '/api/admin/vms/unassign'; body = { id: vm.id };
    } else {
      const action = button.dataset.power;
      const labels = { restart: 'Restart', shutdown: 'Shut down', 'force-stop': 'Force stop' };
      if (action !== 'start' && !confirm(`${labels[action]} ${vm.name}? ${action === 'force-stop' ? 'This immediately cuts power and may damage unsaved files.' : 'Save any open work first.'}`)) return;
      url = `/api/vm/computers/${encodeURIComponent(vm.id)}/power`; body = { action };
    }
    const isDeleteAction = button.hasAttribute('data-delete') || button.hasAttribute('data-force-delete');
    pending.add(vm.id); renderFleet(); status('fleet-status', isDeleteAction ? 'Deleting computer...' : 'Updating computer...');
    try {
      const res = await api(url, body);
      if (isDeleteAction) {
        failedDeletes.delete(vm.id);
      }
      status('fleet-status', res.message || 'Request accepted.', 'success');
      setTimeout(() => { pending.delete(vm.id); load(); }, isDeleteAction ? 1000 : 6000);
    }
    catch (error) {
      if (button.hasAttribute('data-delete')) {
        failedDeletes.add(vm.id);
      }
      pending.delete(vm.id); renderFleet(); status('fleet-status', error.message, 'error');
    }
  });
  $('refresh-button').addEventListener('click', load);
  setupChartInteraction();
  load();
  const timer = setInterval(() => { if (!document.hidden && !creating && !assigning && !pending.size) load(); }, 20000);
  window.addEventListener('pagehide', () => { clearInterval(timer); $('create-password').value = ''; }, { once: true });
})();
