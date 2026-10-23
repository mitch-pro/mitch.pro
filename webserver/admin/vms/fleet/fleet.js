import RFB from '/novnc/core/rfb.js';

(function () {
  'use strict';

  const $ = id => document.getElementById(id);
  const esc = str => String(str ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  function getAdminHeaders(extra = {}) {
    const h = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1', ...extra };
    const pass = sessionStorage.getItem('admin_passphrase');
    if (pass) h['X-Admin-Passphrase'] = pass;
    return h;
  }

  async function api(url, body) {
    const res = await fetch(url, {
      credentials: 'same-origin',
      cache: 'no-store',
      headers: getAdminHeaders(),
      ...(body ? { method: 'POST', body: JSON.stringify(body) } : {})
    });
    const data = await res.json().catch(() => ({}));
    if (res.status === 401) {
      location.href = '/enroll/?next=' + encodeURIComponent(location.pathname);
      throw new Error('Sign in required.');
    }
    if (!res.ok) {
      throw new Error(data.message || data.error || 'Server request failed.');
    }
    return data;
  }

  // State
  let fleetList = [];
  const vmClients = new Map(); // vmId -> { vm, rfb, connected, connecting, viewportEl, cardEl, jiggleState: false }
  let jigglerActive = true;
  let jigglerTimer = null;
  let currentFocusedVmId = null;
  let statusPollTimer = null;

  // UI Elements
  const gridView = $('grid-view');
  const singleView = $('single-view');
  const btnBackToGrid = $('btn-back-to-grid');
  const jigglerPill = $('jiggler-pill');
  const jigglerDot = $('jiggler-dot');
  const btnToggleJiggler = $('btn-toggle-jiggler');
  const btnPowerOnAll = $('btn-power-on-all');
  const btnPowerOffAll = $('btn-power-off-all');
  const btnRefresh = $('btn-refresh');
  const singleVmName = $('single-vm-name');
  const singleVmStatus = $('single-vm-status');
  const singleScreenContainer = $('single-screen-container');
  const btnSingleRestart = $('btn-single-restart');
  const btnSinglePower = $('btn-single-power');
  const btnSingleFullscreen = $('btn-single-fullscreen');
  const fleetGlobalStatus = $('fleet-global-status');

  function setGlobalStatus(msg, isError = false) {
    fleetGlobalStatus.textContent = msg;
    fleetGlobalStatus.style.color = isError ? 'var(--danger)' : 'var(--muted)';
  }

  // Update counts
  function updateCounts() {
    $('count-total').textContent = fleetList.length;
    const running = fleetList.filter(vm => vm.status === 'running').length;
    $('count-running').textContent = running;
    $('count-stopped').textContent = fleetList.length - running;
  }

  // Mouse Jiggler logic
  function runMouseJiggler() {
    if (!jigglerActive) return;

    let jiggledCount = 0;
    for (const [id, client] of vmClients.entries()) {
      if (client.rfb && client.connected) {
        client.jiggleState = !client.jiggleState;
        const x = client.jiggleState ? 200 : 204;
        const y = 200;
        try {
          // Send pointer move without buttons
          if (typeof client.rfb._sendMouse === 'function') {
            client.rfb._sendMouse(x, y, 0);
            jiggledCount++;
          }
        } catch (_) {}
      }
    }

    if (jiggledCount > 0 && jigglerDot) {
      jigglerDot.classList.add('pulse');
      setTimeout(() => jigglerDot.classList.remove('pulse'), 400);
    }
  }

  function startJiggler() {
    if (jigglerTimer) clearInterval(jigglerTimer);
    jigglerTimer = setInterval(runMouseJiggler, 10000);
  }

  btnToggleJiggler.addEventListener('click', () => {
    jigglerActive = !jigglerActive;
    if (jigglerActive) {
      jigglerPill.classList.add('active');
      btnToggleJiggler.textContent = 'Pause Jiggler';
      startJiggler();
    } else {
      jigglerPill.classList.remove('active');
      btnToggleJiggler.textContent = 'Resume Jiggler';
      if (jigglerTimer) clearInterval(jigglerTimer);
    }
  });

  // Connect RFB to a VM
  async function connectVm(vmId, targetContainer) {
    const client = vmClients.get(vmId);
    if (!client) return;
    if (client.connecting || client.connected) return;

    client.connecting = true;
    const overlay = client.cardEl.querySelector('.screen-overlay');
    if (overlay) {
      overlay.classList.remove('hidden');
      overlay.innerHTML = '<div class="spinner"></div><p>Connecting to desktop...</p>';
    }

    try {
      const session = await api(`/api/vm/computers/${encodeURIComponent(vmId)}/desktop-session`, {});
      if (!session.socketPath) throw new Error('No desktop socket session returned.');

      const url = new URL(session.socketPath, location.origin);
      url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';

      const container = targetContainer || client.viewportEl;
      container.innerHTML = '';

      const rfb = new RFB(container, url.href, {
        shared: true,
        credentials: session.credentials
      });

      rfb.scaleViewport = true;
      rfb.resizeSession = false;
      rfb.clipViewport = false;
      rfb.showDotCursor = false;
      rfb.background = '#000000';

      rfb.addEventListener('connect', () => {
        client.connected = true;
        client.connecting = false;
        if (overlay) overlay.classList.add('hidden');
        const badge = client.cardEl.querySelector('.status-indicator');
        if (badge) {
          badge.className = 'status-indicator running';
        }
      });

      rfb.addEventListener('disconnect', () => {
        client.connected = false;
        client.connecting = false;
        if (overlay && currentFocusedVmId !== vmId) {
          overlay.classList.remove('hidden');
          overlay.innerHTML = '<p>Disconnected</p><button type="button" class="btn-reconnect">Reconnect</button>';
          overlay.querySelector('.btn-reconnect')?.addEventListener('click', e => {
            e.stopPropagation();
            connectVm(vmId);
          });
        }
      });

      client.rfb = rfb;
    } catch (err) {
      client.connecting = false;
      client.connected = false;
      if (overlay) {
        overlay.classList.remove('hidden');
        overlay.innerHTML = `<p>${esc(err.message)}</p><button type="button" class="btn-retry">Retry</button>`;
        overlay.querySelector('.btn-retry')?.addEventListener('click', e => {
          e.stopPropagation();
          connectVm(vmId);
        });
      }
    }
  }

  function disconnectVm(vmId) {
    const client = vmClients.get(vmId);
    if (!client || !client.rfb) return;
    try {
      client.rfb.disconnect();
    } catch (_) {}
    client.rfb = null;
    client.connected = false;
    client.connecting = false;
  }

  // Switch to single focused view
  function openSingleView(vmId) {
    const client = vmClients.get(vmId);
    if (!client) return;

    currentFocusedVmId = vmId;
    gridView.style.display = 'none';
    singleView.classList.add('active');

    singleVmName.textContent = client.vm.friendlyName || client.vm.hostname;
    singleVmStatus.textContent = client.vm.status === 'running' ? 'Running' : 'Stopped';
    singleVmStatus.className = `badge ${client.vm.status === 'running' ? 'owner' : ''}`;

    singleScreenContainer.innerHTML = '';

    // Disconnect and reattach to the larger container
    disconnectVm(vmId);
    connectVm(vmId, singleScreenContainer);
  }

  function closeSingleView() {
    if (!currentFocusedVmId) return;
    const vmId = currentFocusedVmId;
    currentFocusedVmId = null;

    disconnectVm(vmId);
    singleScreenContainer.innerHTML = '';
    singleView.classList.remove('active');
    gridView.style.display = 'grid';

    // Reconnect in grid view
    const client = vmClients.get(vmId);
    if (client && client.vm.status === 'running') {
      connectVm(vmId, client.viewportEl);
    }
  }

  btnBackToGrid.addEventListener('click', closeSingleView);

  // Render the 24 cards
  function renderFleet() {
    if (fleetList.length === 0) {
      gridView.innerHTML = `
        <div class="empty-state" style="grid-column: 1 / -1;">
          <h2>No ChromeOS fleet VMs found</h2>
          <p>You haven't provisioned the 24 ChromeOS fleet VMs yet.</p>
          <button type="button" id="btn-provision-now" class="primary" style="margin-top: 12px;">Provision 24 ChromeOS VMs</button>
        </div>
      `;
      $('btn-provision-now')?.addEventListener('click', async () => {
        setGlobalStatus('Provisioning 24 ChromeOS VMs, please wait...');
        try {
          await api('/api/admin/vms/fleet/create-chromeos', {});
          setGlobalStatus('Successfully provisioned! Loading fleet...');
          await loadFleet();
        } catch (err) {
          setGlobalStatus(err.message, true);
        }
      });
      return;
    }

    gridView.innerHTML = '';

    for (const vm of fleetList) {
      const card = document.createElement('div');
      card.className = 'screen-card';
      card.id = `card-${esc(vm.id)}`;

      const isRunning = vm.status === 'running';

      card.innerHTML = `
        <div class="screen-card-header">
          <div class="screen-card-title">
            <span class="status-indicator ${isRunning ? 'running' : 'stopped'}"></span>
            <span>${esc(vm.friendlyName || vm.hostname)}</span>
          </div>
          <div class="screen-card-actions">
            <button type="button" class="btn-card-power">${isRunning ? 'Stop' : 'Start'}</button>
            <button type="button" class="btn-card-focus" title="Expand to individual screen">&#x26F6; Focus</button>
          </div>
        </div>
        <div class="screen-viewport" id="viewport-${esc(vm.id)}">
          <div class="screen-overlay ${isRunning ? 'hidden' : ''}">
            <p>${isRunning ? 'Connecting...' : 'Computer is stopped'}</p>
            <button type="button" class="btn-overlay-start primary">${isRunning ? 'Connect' : 'Start computer'}</button>
          </div>
        </div>
      `;

      const viewportEl = card.querySelector(`#viewport-${CSS.escape(vm.id)}`);
      const btnFocus = card.querySelector('.btn-card-focus');
      const btnCardPower = card.querySelector('.btn-card-power');
      const btnOverlayStart = card.querySelector('.btn-overlay-start');

      // Click card viewport or focus button to open single view
      btnFocus.addEventListener('click', e => {
        e.stopPropagation();
        openSingleView(vm.id);
      });

      viewportEl.addEventListener('click', () => {
        openSingleView(vm.id);
      });

      // Individual power action
      const togglePower = async (e) => {
        e.stopPropagation();
        const run = vm.status === 'running';
        const action = run ? 'shutdown' : 'start';
        setGlobalStatus(`${run ? 'Stopping' : 'Starting'} ${vm.friendlyName || vm.hostname}...`);
        try {
          await api(`/api/vm/computers/${encodeURIComponent(vm.id)}/power`, { action });
          vm.status = run ? 'stopped' : 'running';
          renderFleet();
        } catch (err) {
          setGlobalStatus(err.message, true);
        }
      };

      btnCardPower.addEventListener('click', togglePower);
      btnOverlayStart.addEventListener('click', togglePower);

      gridView.appendChild(card);

      vmClients.set(vm.id, {
        vm,
        rfb: null,
        connected: false,
        connecting: false,
        cardEl: card,
        viewportEl,
        jiggleState: false
      });

      // If running, auto-connect RFB
      if (isRunning && currentFocusedVmId !== vm.id) {
        connectVm(vm.id, viewportEl);
      }
    }
  }

  // Load fleet from server
  async function loadFleet() {
    setGlobalStatus('Loading fleet data...');
    try {
      const data = await api('/api/admin/vms/fleet/chromeos');
      fleetList = data.vms || [];
      updateCounts();
      renderFleet();
      setGlobalStatus('');
    } catch (err) {
      setGlobalStatus(err.message, true);
    }
  }

  // Batch actions
  btnPowerOnAll.addEventListener('click', async () => {
    btnPowerOnAll.disabled = true;
    setGlobalStatus('Turning on all 24 ChromeOS VMs...');
    try {
      await api('/api/admin/vms/fleet/power', { action: 'start' });
      setGlobalStatus('Turn on command sent to all 24 VMs. Refreshing status...');
      setTimeout(loadFleet, 3000);
    } catch (err) {
      setGlobalStatus(err.message, true);
    } finally {
      btnPowerOnAll.disabled = false;
    }
  });

  btnPowerOffAll.addEventListener('click', async () => {
    if (!confirm('Power off all 24 ChromeOS fleet VMs?')) return;
    btnPowerOffAll.disabled = true;
    setGlobalStatus('Shutting down all 24 ChromeOS VMs...');
    try {
      await api('/api/admin/vms/fleet/power', { action: 'shutdown' });
      setGlobalStatus('Shutdown command sent to all 24 VMs.');
      // Disconnect all
      for (const id of vmClients.keys()) {
        disconnectVm(id);
      }
      setTimeout(loadFleet, 3000);
    } catch (err) {
      setGlobalStatus(err.message, true);
    } finally {
      btnPowerOffAll.disabled = false;
    }
  });

  btnRefresh.addEventListener('click', () => {
    loadFleet();
  });

  // Single mode buttons
  btnSingleFullscreen.addEventListener('click', () => {
    if (!document.fullscreenElement) {
      singleScreenContainer.requestFullscreen().catch(() => {});
    } else {
      document.exitFullscreen().catch(() => {});
    }
  });

  btnSinglePower.addEventListener('click', async () => {
    if (!currentFocusedVmId) return;
    const client = vmClients.get(currentFocusedVmId);
    if (!client) return;
    btnSinglePower.disabled = true;
    try {
      await api(`/api/vm/computers/${encodeURIComponent(currentFocusedVmId)}/power`, { action: 'shutdown' });
      closeSingleView();
      loadFleet();
    } catch (err) {
      alert(err.message);
    } finally {
      btnSinglePower.disabled = false;
    }
  });

  btnSingleRestart.addEventListener('click', async () => {
    if (!currentFocusedVmId) return;
    btnSingleRestart.disabled = true;
    try {
      await api(`/api/vm/computers/${encodeURIComponent(currentFocusedVmId)}/power`, { action: 'restart' });
      alert('Restart initiated.');
    } catch (err) {
      alert(err.message);
    } finally {
      btnSingleRestart.disabled = false;
    }
  });

  // Start Mouse Jiggler loop
  startJiggler();

  // Initial load
  loadFleet();
})();
