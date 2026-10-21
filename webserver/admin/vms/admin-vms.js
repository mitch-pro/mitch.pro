(function () {
  'use strict';
  const $ = id => document.getElementById(id);
  const esc = value => String(value ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const bytes = value => Number(value) ? `${(Number(value) / 1073741824).toLocaleString(undefined, { maximumFractionDigits: 1 })} GB` : '\u2014';
  const pct = (used, total) => total ? Math.max(0, Math.min(100, Math.round(used / total * 100))) : 0;
  const pending = new Set(), failedDeletes = new Set(), selectedForBulk = new Set();
  let overview = null, loading = false, creating = false, assigning = false;
  let fleetFilter = 'all', fleetSearch = '';
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
    const serviceState = $('service-state');
    if (serviceState) {
      serviceState.className = 'service-state offline';
      const label = serviceState.querySelector('span');
      if (label) label.textContent = isNewSetup ? 'Passphrase setup required' : 'Passphrase required';
    }
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
  function matchesFleetFilter(vm) {
    if (fleetFilter === 'all') return true;
    if (fleetFilter === 'running') return vm.status === 'running';
    if (fleetFilter === 'stopped') return vm.status === 'stopped';
    if (fleetFilter === 'provisioning') return vm.status === 'provisioning';
    if (fleetFilter === 'failed') return vm.status === 'provisioning-failed' || failedDeletes.has(vm.id);
    return true;
  }
  function matchesFleetSearch(vm) {
    if (!fleetSearch) return true;
    const q = fleetSearch.toLowerCase();
    return [vm.id, vm.hostname, vm.ownerEmail, vm.name].some(v => String(v || '').toLowerCase().includes(q));
  }
  function lastVmPowerError(vmId) {
    const rows = overview.audit || [];
    for (const row of rows) {
      if (row.vmRecordId !== vmId || row.success !== false || !String(row.action || '').startsWith('VM_')) continue;
      if (Date.now() - new Date(row.ts).getTime() > 10 * 60 * 1000) return null;
      try {
        const detail = JSON.parse(row.details || '{}');
        if (detail.message) return detail.message;
      } catch (_) { /* ignore malformed detail */ }
      return null;
    }
    return null;
  }
  function renderFleet() {
    const allRows = (overview.computers || []).filter(vm => vm.assignmentStatus !== 'unassigned');
    for (const id of [...selectedForBulk]) if (!allRows.some(vm => vm.id === id && vm.status === 'running')) selectedForBulk.delete(id);
    const rows = allRows.filter(vm => matchesFleetFilter(vm) && matchesFleetSearch(vm));
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
      const selectBox = canAccess ? `<label class="fleet-select" title="Select for bulk reboot"><input type="checkbox" class="fleet-select-box" data-select="${esc(vm.id)}" ${running ? '' : 'disabled'}${selectedForBulk.has(vm.id) ? ' checked' : ''}> Select</label>` : '';
      const powerError = !busy ? lastVmPowerError(vm.id) : null;
      const errorBanner = powerError
        ? `<div class="fleet-power-error" style="grid-column:1/-1;color:#ffb3b3;font-size:.76rem;padding-top:4px;">⚠️ ${esc(powerError)} ${canAccess ? `<button data-retry-force-stop style="margin-left:6px;">Retry Force Stop</button>` : ''}</div>`
        : '';
      return `<article class="fleet-item" data-id="${esc(vm.id)}"><div class="fleet-identity"><strong>${esc(vm.name)}</strong><small>${esc(vm.operatingSystem || 'Linux desktop')}</small></div><div class="fleet-owner"><strong title="${esc(vm.ownerEmail)}">${esc(vm.ownerEmail)}</strong><small>${esc(vm.hostname || 'No hostname')}</small></div><span class="fleet-state ${running ? 'running' : ''}">${busy ? 'Updating...' : running ? 'Running' : stopped ? 'Offline' : esc(vm.status)}</span>${inUse ? `<span class="fleet-in-use" style="background:#15803d; color:#f0fdf4; font-size:0.75rem; font-weight:700; padding:2px 8px; border-radius:999px; margin-left:4px;" title="Active desktop session">👤 In Use (${esc(activeUserLabel)})</span>` : ''}<span class="fleet-resources">${esc(vm.cpuCores)} CPU &middot; ${bytes(vm.memoryTotal)}<small>${bytes(vm.diskTotal)} disk</small></span><span class="fleet-address">${esc(vm.ipAddress || 'No IP yet')}</span><div class="fleet-actions">${selectBox}${openDesktopAction}${powerButtons}<details class="vm-actions-menu"><summary>Manage</summary><div><button data-unassign class="unassign" ${busy ? 'disabled' : ''}>Unassign</button><button data-resources ${busy ? 'disabled' : ''}>Edit resources</button><button data-vm-ban ${busy ? 'disabled' : ''}>${vm.ownerVmBanned ? 'Restore VM access' : 'Restrict VM access'}</button><button data-byo-ban ${busy ? 'disabled' : ''}>${vm.ownerByoBanned ? 'Restore BYO-OS' : 'Restrict BYO-OS'}</button><button data-delete class="danger" ${busy ? 'disabled' : ''}>Delete</button>${hasFailedDelete ? `<button data-force-delete class="danger" style="background:#ef4444; color:#fff; border-color:#ef4444; font-weight:700;" ${busy ? 'disabled' : ''}>⚠️ Force Delete</button>` : ''}</div></details></div>${errorBanner}</article>`;
    }).join('') : '<p class="empty">No customer computers match.</p>';
    $('bulk-reboot').disabled = selectedForBulk.size === 0;
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
  function auditErrorDetail(row) {
    if (row.success) return '';
    try {
      const detail = JSON.parse(row.details || '{}');
      return detail.message || '';
    } catch (_) { return ''; }
  }
  function renderAudit() {
    const rows = overview.audit || [];
    $('audit-list').innerHTML = rows.length ? rows.map(row => {
      const detail = auditErrorDetail(row);
      return `<div class="audit-row${detail ? ' has-detail' : ''}"><time>${esc(new Date(row.ts).toLocaleString('en-US', { dateStyle: 'short', timeStyle: 'short' }))}</time><strong>${esc(String(row.action || '').replaceAll('_', ' '))}</strong><span>${esc(row.actorEmail)}${row.ownerEmail ? ` &rarr; ${esc(row.ownerEmail)}` : ''}</span><span class="${row.success ? '' : 'failed'}">${row.success ? 'Success' : 'Failed'}</span>${detail ? `<div class="audit-detail">${esc(detail)}</div>` : ''}</div>`;
    }).join('') : '<p class="empty">No activity yet.</p>';
  }

  let activeHoverHour = -1;

  async function load() {
    if (loading) return;
    loading = true; $('refresh-button').disabled = true;
    try {
      overview = await api('/api/admin/vms/overview');
      $('service-state').className = `service-state ${overview.serviceAvailable ? 'online' : 'offline'}`;
      $('service-state').querySelector('span').textContent = overview.serviceAvailable ? 'Computer service online' : 'Computer service unavailable';
      renderCapacity(); renderForms(); renderFleet(); renderAudit();
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
            if (queue.length) {
              queue.forEach(p => p.resolve(pass));
            } else {
              load();
            }
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
  const DESIRED_STATUS_AFTER_ACTION = { start: 'running', restart: 'running', shutdown: 'stopped', 'force-stop': 'stopped' };
  async function pollUntilSettled(vmId, desiredStatus) {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 1500));
      await load();
      const vm = overview?.computers?.find(row => row.id === vmId);
      if (!vm || vm.status === desiredStatus) break;
    }
    pending.delete(vmId);
    renderFleet();
  }
  async function runPowerAction(vmId, action) {
    pending.add(vmId); renderFleet(); status('fleet-status', 'Updating computer...');
    try {
      const res = await api(`/api/vm/computers/${encodeURIComponent(vmId)}/power`, { action });
      status('fleet-status', res.message || 'Request accepted.', 'success');
      await pollUntilSettled(vmId, DESIRED_STATUS_AFTER_ACTION[action]);
    } catch (error) {
      pending.delete(vmId); renderFleet(); status('fleet-status', error.message, 'error');
    }
  }
  $('fleet-list').addEventListener('click', async event => {
    const button = event.target.closest('button'), item = button?.closest('[data-id]');
    if (!item || button.disabled || pending.has(item.dataset.id)) return;
    const vm = overview?.computers.find(row => row.id === item.dataset.id); if (!vm) return;
    if (button.hasAttribute('data-retry-force-stop')) {
      if (!confirm(`Retry force stop on ${vm.name}? This immediately cuts power and may damage unsaved files.`)) return;
      await runPowerAction(vm.id, 'force-stop');
      return;
    }
    if (button.dataset.power) {
      const action = button.dataset.power;
      const labels = { restart: 'Restart', shutdown: 'Shut down', 'force-stop': 'Force stop' };
      if (action !== 'start' && !confirm(`${labels[action]} ${vm.name}? ${action === 'force-stop' ? 'This immediately cuts power and may damage unsaved files.' : 'Save any open work first.'}`)) return;
      await runPowerAction(vm.id, action);
      return;
    }
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
    } else if (button.hasAttribute('data-resources')) {
      resourceVm = vm;
      $('resource-machine').textContent = vm.name;
      $('resource-cpu').value = vm.cpuCores;
      $('resource-memory').value = Math.round(vm.memoryMb || vm.memoryTotal / 1048576);
      $('resource-disk').value = Math.round(vm.diskGb || vm.diskTotal / 1073741824);
      $('resource-disk').min = $('resource-disk').value;
      $('resource-status').textContent = '';
      $('resource-dialog').showModal();
      return;
    } else if (button.hasAttribute('data-vm-ban')) {
      if (!confirm(`${vm.ownerVmBanned ? 'Restore' : 'Restrict'} VM access for ${vm.ownerEmail}?`)) return;
      url = '/api/admin/vm-ban'; body = { email: vm.ownerEmail, banned: !vm.ownerVmBanned };
    } else if (button.hasAttribute('data-byo-ban')) {
      if (!confirm(`${vm.ownerByoBanned ? 'Restore' : 'Restrict'} BYO-OS access for ${vm.ownerEmail}?`)) return;
      url = '/api/admin/byo-os-ban'; body = { email: vm.ownerEmail, banned: !vm.ownerByoBanned };
    } else {
      return;
    }
    const isDeleteAction = button.hasAttribute('data-delete') || button.hasAttribute('data-force-delete');
    pending.add(vm.id); renderFleet(); status('fleet-status', isDeleteAction ? 'Deleting computer...' : 'Updating computer...');
    try {
      const res = await api(url, body);
      if (isDeleteAction) {
        failedDeletes.delete(vm.id);
      }
      status('fleet-status', res.message || 'Request accepted.', 'success');
      setTimeout(() => { pending.delete(vm.id); load(); }, 1000);
    }
    catch (error) {
      if (button.hasAttribute('data-delete')) {
        failedDeletes.add(vm.id);
      }
      pending.delete(vm.id); renderFleet(); status('fleet-status', error.message, 'error');
    }
  });
  $('fleet-list').addEventListener('change', event => {
    const box = event.target.closest('.fleet-select-box');
    if (!box) return;
    if (box.checked) selectedForBulk.add(box.dataset.select);
    else selectedForBulk.delete(box.dataset.select);
    $('bulk-reboot').disabled = selectedForBulk.size === 0;
  });
  $('fleet-filters').addEventListener('click', event => {
    const button = event.target.closest('.fleet-filter');
    if (!button) return;
    fleetFilter = button.dataset.filter;
    [...$('fleet-filters').children].forEach(b => b.classList.toggle('active', b === button));
    if (overview) renderFleet();
  });
  $('fleet-search').addEventListener('input', event => {
    fleetSearch = event.target.value.trim();
    if (overview) renderFleet();
  });
  $('bulk-reboot').addEventListener('click', async () => {
    const ids = [...selectedForBulk];
    if (!ids.length || !confirm(`Restart ${ids.length} selected computer${ids.length === 1 ? '' : 's'}? Save any open work first.`)) return;
    status('fleet-status', `Restarting ${ids.length} computer${ids.length === 1 ? '' : 's'}...`);
    selectedForBulk.clear();
    await Promise.all(ids.map(id => runPowerAction(id, 'restart')));
    status('fleet-status', 'Bulk restart complete.', 'success');
  });
  $('bulk-shutdown-inactive').addEventListener('click', async () => {
    const inactive = (overview?.computers || []).filter(vm =>
      vm.assignmentStatus !== 'unassigned' && vm.status === 'running' && !(Array.isArray(vm.activeUsers) && vm.activeUsers.length) && !pending.has(vm.id));
    if (!inactive.length) { status('fleet-status', 'No inactive running computers to shut down.'); return; }
    if (!confirm(`Shut down ${inactive.length} running computer${inactive.length === 1 ? '' : 's'} with no active desktop session?`)) return;
    status('fleet-status', `Shutting down ${inactive.length} inactive computer${inactive.length === 1 ? '' : 's'}...`);
    await Promise.all(inactive.map(vm => runPowerAction(vm.id, 'shutdown')));
    status('fleet-status', 'Inactive computers shut down.', 'success');
  });
  $('audit-list').addEventListener('click', event => {
    const row = event.target.closest('.audit-row.has-detail');
    if (row) row.classList.toggle('expanded');
  });
  $('refresh-button').addEventListener('click', load);
  let resourceVm;
  function focusResourceAction() {
    if (resourceVm) document.querySelector(`[data-id="${CSS.escape(resourceVm.id)}"] .vm-actions-menu > summary`)?.focus({preventScroll:true});
  }
  $('resource-dialog').addEventListener('close', focusResourceAction);
  $('resource-cancel').addEventListener('click', () => $('resource-dialog').close());
  $('resource-form').addEventListener('submit', async event => {
    event.preventDefault();
    if (!resourceVm || pending.has(resourceVm.id)) return;
    const id = resourceVm.id;
    pending.add(id);
    const submit = event.submitter; submit.disabled = true;
    $('resource-status').textContent = 'Saving…';
    try {
      await api('/api/admin/vms/resources', { id, cpuCores: Number($('resource-cpu').value), memoryMb: Number($('resource-memory').value), diskGb: Number($('resource-disk').value) });
      $('resource-dialog').close(); status('fleet-status', 'Resources updated.', 'success');
    } catch (error) { $('resource-status').textContent = error.message; }
    finally { pending.delete(id); submit.disabled = false; await load(); if (!$('resource-dialog').open) focusResourceAction(); }
  });
  load();
  const timer = setInterval(() => {
    const locked = $('passphrase-lock-box')?.style.display === 'block';
    if (!document.hidden && !locked && !creating && !assigning && !pending.size) load();
  }, 20000);
  window.addEventListener('pagehide', () => { clearInterval(timer); $('create-password').value = ''; }, { once: true });
})();
