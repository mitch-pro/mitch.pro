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
  let mirrorActive = false;

  // UI Elements
  const gridView = $('grid-view');
  const singleView = $('single-view');
  const btnBackToGrid = $('btn-back-to-grid');
  const jigglerPill = $('jiggler-pill');
  const jigglerDot = $('jiggler-dot');
  const btnToggleJiggler = $('btn-toggle-jiggler');
  const btnToggleMirror = $('btn-toggle-mirror');
  const btnSingleToggleMirror = $('btn-single-toggle-mirror');
  const broadcastBar = $('broadcast-bar');
  const broadcastTextInput = $('broadcast-text-input');
  const btnBroadcastType = $('btn-broadcast-type');
  const btnBroadcastEnter = $('btn-broadcast-enter');
  const btnBroadcastTab = $('btn-broadcast-tab');
  const btnBroadcastBackspace = $('btn-broadcast-backspace');
  const broadcastX = $('broadcast-x');
  const broadcastY = $('broadcast-y');
  const btnBroadcastClick = $('btn-broadcast-click');
  const broadcastIndicator = $('broadcast-indicator');
  const mirrorStatusText = $('mirror-status-text');
  const coordsDisplayPill = $('coords-display-pill');
  const coordsText = $('coords-text');
  const singleCoordsDisplay = $('single-coords-display');
  const singleCoordsText = $('single-coords-text');
  const btnPowerOnAll = $('btn-power-on-all');
  const btnPowerOffAll = $('btn-power-off-all');
  const btnForceStopAll = $('btn-force-stop-all');
  const btnDeleteAllFleet = $('btn-delete-all-fleet');
  const btnDeleteLocalFleet = $('btn-delete-local-fleet');
  const btnRefresh = $('btn-refresh');
  const singleVmName = $('single-vm-name');
  const singleVmStatus = $('single-vm-status');
  const singleScreenContainer = $('single-screen-container');
  const btnSingleRestart = $('btn-single-restart');
  const btnSinglePower = $('btn-single-power');
  const btnSingleDelete = $('btn-single-delete');
  const btnSingleFullscreen = $('btn-single-fullscreen');
  const fleetGlobalStatus = $('fleet-global-status');

  function setGlobalStatus(msg, isError = false) {
    fleetGlobalStatus.textContent = msg;
    fleetGlobalStatus.style.color = isError ? 'var(--danger)' : 'var(--muted)';
  }

  function toggleMirroring(force) {
    mirrorActive = typeof force === 'boolean' ? force : !mirrorActive;
    const label = mirrorActive ? '🪞 Mirror Controls: ON' : '🪞 Mirror Controls: OFF';
    if (btnToggleMirror) btnToggleMirror.textContent = label;
    if (btnSingleToggleMirror) btnSingleToggleMirror.textContent = label;
    if (broadcastBar) broadcastBar.classList.toggle('mirroring-active', mirrorActive);
    if (broadcastIndicator) broadcastIndicator.classList.toggle('active', mirrorActive);
    if (mirrorStatusText) {
      mirrorStatusText.textContent = mirrorActive ? 'Mirroring is ACTIVE (All 24 VMs)' : 'Mirroring is OFF';
    }
  }

  btnToggleMirror?.addEventListener('click', () => toggleMirroring());
  btnSingleToggleMirror?.addEventListener('click', () => toggleMirroring());

  async function broadcastTypeText(text) {
    if (!text) return;
    setGlobalStatus(`Typing "${text}" across all connected Chromebooks...`);
    for (let i = 0; i < text.length; i++) {
      const code = text.charCodeAt(i);
      for (const client of vmClients.values()) {
        if (client.rfb && client.connected && client.rfb._rfbConnectionState === 'connected') {
          try {
            client.rfb.sendKey(code, null, true);
            client.rfb.sendKey(code, null, false);
          } catch (_) {}
        }
      }
      await new Promise(r => setTimeout(r, 25));
    }
    setGlobalStatus('Finished typing broadcast text.');
  }

  function broadcastKey(keysym, code) {
    for (const client of vmClients.values()) {
      if (client.rfb && client.connected && client.rfb._rfbConnectionState === 'connected') {
        try {
          client.rfb.sendKey(keysym, code, true);
          client.rfb.sendKey(keysym, code, false);
        } catch (_) {}
      }
    }
  }

  function broadcastClick(x, y) {
    setGlobalStatus(`Clicking at (${x}, ${y}) across all connected Chromebooks...`);
    for (const client of vmClients.values()) {
      if (client.rfb && client.connected && client.rfb._rfbConnectionState === 'connected') {
        try {
          RFB.messages.pointerEvent(client.rfb._sock, x, y, 0);
          RFB.messages.pointerEvent(client.rfb._sock, x, y, 1);
          setTimeout(() => {
            try {
              RFB.messages.pointerEvent(client.rfb._sock, x, y, 0);
            } catch (_) {}
          }, 80);
        } catch (_) {}
      }
    }
  }

  btnBroadcastType?.addEventListener('click', () => {
    const text = broadcastTextInput.value;
    if (text) {
      broadcastTypeText(text);
      broadcastTextInput.value = '';
    }
  });

  broadcastTextInput?.addEventListener('keydown', e => {
    if (e.key === 'Enter') {
      btnBroadcastType?.click();
    }
  });

  btnBroadcastEnter?.addEventListener('click', () => broadcastKey(0xff0d, 'Enter'));
  btnBroadcastTab?.addEventListener('click', () => broadcastKey(0xff09, 'Tab'));
  btnBroadcastBackspace?.addEventListener('click', () => broadcastKey(0xff08, 'Backspace'));
  btnBroadcastClick?.addEventListener('click', () => {
    const x = Number(broadcastX.value) || 500;
    const y = Number(broadcastY.value) || 300;
    broadcastClick(x, y);
  });

  // Coordinates tracking & feedback
  let lastRecordedCoords = null;

  function updateLiveCoords(fbX, fbY) {
    if (typeof fbX !== 'number' || typeof fbY !== 'number' || isNaN(fbX) || isNaN(fbY)) return;
    if (singleCoordsText) {
      singleCoordsText.textContent = `X: ${fbX}, Y: ${fbY}`;
    }
  }

  function handleCoordinateClick(fbX, fbY, visualPoint, vmId) {
    if (typeof fbX !== 'number' || typeof fbY !== 'number' || isNaN(fbX) || isNaN(fbY)) return;
    lastRecordedCoords = { x: fbX, y: fbY };

    // Update broadcast inputs
    if (broadcastX) broadcastX.value = fbX;
    if (broadcastY) broadcastY.value = fbY;

    // Update coordinate labels
    const label = `Clicked: (${fbX}, ${fbY})`;
    if (coordsText) coordsText.textContent = label;
    if (singleCoordsText) singleCoordsText.textContent = `X: ${fbX}, Y: ${fbY}`;

    // Flash highlights
    coordsDisplayPill?.classList.add('highlight');
    singleCoordsDisplay?.classList.add('highlight');
    setTimeout(() => {
      coordsDisplayPill?.classList.remove('highlight');
      singleCoordsDisplay?.classList.remove('highlight');
    }, 700);

    // Show floating bubble & ripple if visual point given
    if (visualPoint && visualPoint.container) {
      const bubble = document.createElement('div');
      bubble.className = 'click-coord-bubble';
      bubble.textContent = `(${fbX}, ${fbY})`;
      bubble.style.left = `${visualPoint.localX}px`;
      bubble.style.top = `${visualPoint.localY}px`;
      visualPoint.container.appendChild(bubble);

      const ripple = document.createElement('div');
      ripple.className = 'click-coord-ripple';
      ripple.style.left = `${visualPoint.localX}px`;
      ripple.style.top = `${visualPoint.localY}px`;
      visualPoint.container.appendChild(ripple);

      setTimeout(() => {
        bubble.remove();
        ripple.remove();
      }, 1900);
    }

    // Try to copy to clipboard
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(`${fbX}, ${fbY}`).catch(() => {});
    }

    setGlobalStatus(`📍 Clicked VM coordinates: X=${fbX}, Y=${fbY} (copied to Broadcast Click inputs)`);
  }

  function computeFbCoords(e, container, rfb) {
    if (!container) return null;
    const canvas = container.querySelector('canvas');
    if (!canvas) return null;

    const rect = canvas.getBoundingClientRect();
    const elemX = e.clientX - rect.left;
    const elemY = e.clientY - rect.top;

    let fbX, fbY;
    if (rfb && rfb._display && typeof rfb._display.absX === 'function') {
      fbX = Math.round(rfb._display.absX(elemX));
      fbY = Math.round(rfb._display.absY(elemY));
    } else {
      const scaleX = (canvas.width || 1024) / (rect.width || 1);
      const scaleY = (canvas.height || 768) / (rect.height || 1);
      fbX = Math.round(elemX * scaleX);
      fbY = Math.round(elemY * scaleY);
    }

    const containerRect = container.getBoundingClientRect();
    const localX = e.clientX - containerRect.left;
    const localY = e.clientY - containerRect.top;

    const maxWidth = (rfb && rfb._fbWidth) || canvas.width || 4000;
    const maxHeight = (rfb && rfb._fbHeight) || canvas.height || 4000;

    return {
      fbX: Math.max(0, Math.min(maxWidth, fbX)),
      fbY: Math.max(0, Math.min(maxHeight, fbY)),
      localX,
      localY,
      container
    };
  }

  coordsDisplayPill?.addEventListener('click', () => {
    if (lastRecordedCoords) {
      if (navigator.clipboard?.writeText) {
        navigator.clipboard.writeText(`${lastRecordedCoords.x}, ${lastRecordedCoords.y}`).catch(() => {});
      }
      setGlobalStatus(`Copied coordinates (${lastRecordedCoords.x}, ${lastRecordedCoords.y}) to clipboard.`);
    } else {
      setGlobalStatus('Click anywhere on a VM screen to view its coordinates.');
    }
  });

  singleCoordsDisplay?.addEventListener('click', () => {
    if (lastRecordedCoords) {
      if (navigator.clipboard?.writeText) {
        navigator.clipboard.writeText(`${lastRecordedCoords.x}, ${lastRecordedCoords.y}`).catch(() => {});
      }
      setGlobalStatus(`Copied coordinates (${lastRecordedCoords.x}, ${lastRecordedCoords.y}) to clipboard.`);
    }
  });

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

  // Staggered connection queue to prevent Proxmox pveproxy connection drops (1011)
  const connectQueue = [];
  let isProcessingQueue = false;

  function queueConnectVm(vmId, targetContainer) {
    if (!connectQueue.some(item => item.vmId === vmId)) {
      connectQueue.push({ vmId, targetContainer });
    }
    processConnectQueue();
  }

  async function processConnectQueue() {
    if (isProcessingQueue) return;
    isProcessingQueue = true;
    while (connectQueue.length > 0) {
      const item = connectQueue.shift();
      await connectVm(item.vmId, item.targetContainer);
      await new Promise(r => setTimeout(r, 120));
    }
    isProcessingQueue = false;
  }

  // Connect RFB to a VM
  async function connectVm(vmId, targetContainer) {
    const client = vmClients.get(vmId);
    if (!client) return;
    if (client.connecting || client.connected) return;

    client.connecting = true;
    const overlay = client.cardEl ? client.cardEl.querySelector('.screen-overlay') : null;
    if (overlay && currentFocusedVmId !== vmId) {
      overlay.classList.remove('hidden');
      overlay.innerHTML = '<div class="spinner"></div><p>Connecting to desktop...</p>';
    }

    try {
      const session = await api(`/api/vm/computers/${encodeURIComponent(vmId)}/desktop-session`, {});
      if (!session.socketPath) throw new Error('No desktop socket session returned.');

      const url = new URL(session.socketPath, location.origin);
      url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';

      const container = targetContainer || client.viewportEl;
      if (!container) return;
      container.innerHTML = '';

      const rfb = new RFB(container, url.href, {
        shared: true,
        credentials: session.credentials
      });

      rfb.scaleViewport = true;
      rfb.resizeSession = false;
      rfb.clipViewport = false;
      rfb.showDotCursor = true;
      rfb.background = '#000000';

      if (targetContainer) {
        const cursor = document.createElement('div');
        cursor.id = 'single-local-cursor';
        cursor.className = 'local-cursor';
        cursor.setAttribute('aria-hidden', 'true');
        targetContainer.appendChild(cursor);
      }

      // Hook mouse & keyboard for mirroring across all VMs when active
      const origSendMouse = rfb._sendMouse.bind(rfb);
      rfb._sendMouse = function (x, y, mask) {
        origSendMouse(x, y, mask);
        const fbX = rfb._display ? Math.round(rfb._display.absX(x)) : Math.round(x);
        const fbY = rfb._display ? Math.round(rfb._display.absY(y)) : Math.round(y);

        if (currentFocusedVmId === vmId) {
          updateLiveCoords(fbX, fbY);
          if (mask !== 0) {
            handleCoordinateClick(fbX, fbY, null, vmId);
          }
        }

        if (!mirrorActive) return;
        for (const [otherId, otherClient] of vmClients.entries()) {
          if (otherId === vmId) continue;
          if (otherClient.rfb && otherClient.connected && otherClient.rfb._rfbConnectionState === 'connected') {
            try {
              RFB.messages.pointerEvent(otherClient.rfb._sock, fbX, fbY, mask);
            } catch (_) {}
          }
        }
      };

      const origSendKey = rfb.sendKey.bind(rfb);
      rfb.sendKey = function (keysym, code, down) {
        origSendKey(keysym, code, down);
        if (!mirrorActive) return;
        for (const [otherId, otherClient] of vmClients.entries()) {
          if (otherId === vmId) continue;
          if (otherClient.rfb && otherClient.connected && otherClient.rfb._rfbConnectionState === 'connected') {
            try {
              otherClient.rfb.sendKey(keysym, code, down);
            } catch (_) {}
          }
        }
      };

      rfb.addEventListener('connect', () => {
        client.connected = true;
        client.connecting = false;
        client.retries = 0;
        if (overlay) overlay.classList.add('hidden');
        const badge = client.cardEl?.querySelector('.status-indicator');
        if (badge) {
          badge.className = 'status-indicator running';
        }
        if (currentFocusedVmId === vmId) {
          try { rfb.focus(); } catch (_) {}
        }
      });

      rfb.addEventListener('disconnect', (e) => {
        const wasConnected = client.connected;
        client.connected = false;
        client.connecting = false;

        // Auto-retry up to 3 times with backoff if disconnected during initial boot
        client.retries = (client.retries || 0) + 1;
        if (client.vm.status === 'running' && client.retries <= 3 && !wasConnected) {
          if (overlay && currentFocusedVmId !== vmId) {
            overlay.classList.remove('hidden');
            overlay.innerHTML = `<div class="spinner"></div><p>Waiting for desktop display... (retry ${client.retries}/3)</p>`;
          }
          setTimeout(() => {
            if (client.vm.status === 'running') {
              connectVm(vmId, targetContainer);
            }
          }, 3000);
          return;
        }

        if (overlay && currentFocusedVmId !== vmId) {
          overlay.classList.remove('hidden');
          overlay.innerHTML = '<p>Disconnected</p><button type="button" class="btn-reconnect">Reconnect</button>';
          overlay.querySelector('.btn-reconnect')?.addEventListener('click', ev => {
            ev.stopPropagation();
            client.retries = 0;
            connectVm(vmId);
          });
        }
      });

      client.rfb = rfb;
    } catch (err) {
      client.connecting = false;
      client.connected = false;
      client.retries = (client.retries || 0) + 1;

      if (client.vm.status === 'running' && client.retries <= 3) {
        if (overlay && currentFocusedVmId !== vmId) {
          overlay.classList.remove('hidden');
          overlay.innerHTML = `<div class="spinner"></div><p>Booting desktop... (retry ${client.retries}/3)</p>`;
        }
        setTimeout(() => {
          if (client.vm.status === 'running') {
            connectVm(vmId, targetContainer);
          }
        }, 3000);
        return;
      }

      if (overlay) {
        overlay.classList.remove('hidden');
        overlay.innerHTML = `<p>${esc(err.message)}</p><button type="button" class="btn-retry">Retry</button>`;
        overlay.querySelector('.btn-retry')?.addEventListener('click', ev => {
          ev.stopPropagation();
          client.retries = 0;
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
      queueConnectVm(vmId, client.viewportEl);
    }
  }

  btnBackToGrid.addEventListener('click', closeSingleView);

  singleScreenContainer.addEventListener('mousemove', event => {
    const localCursor = singleScreenContainer.querySelector('#single-local-cursor');
    if (!localCursor || !singleView.classList.contains('active')) return;
    const rect = singleScreenContainer.getBoundingClientRect();
    localCursor.style.left = `${event.clientX - rect.left}px`;
    localCursor.style.top = `${event.clientY - rect.top}px`;
    localCursor.classList.add('is-active');
  }, true);

  singleScreenContainer.addEventListener('pointerdown', event => {
    if (!currentFocusedVmId) return;
    const client = vmClients.get(currentFocusedVmId);
    const coords = computeFbCoords(event, singleScreenContainer, client?.rfb);
    if (coords) {
      handleCoordinateClick(coords.fbX, coords.fbY, coords, currentFocusedVmId);
    }
  }, { capture: true });

  singleScreenContainer.addEventListener('pointermove', event => {
    if (!currentFocusedVmId) return;
    const client = vmClients.get(currentFocusedVmId);
    const coords = computeFbCoords(event, singleScreenContainer, client?.rfb);
    if (coords) {
      updateLiveCoords(coords.fbX, coords.fbY);
    }
  }, { capture: true, passive: true });

  singleScreenContainer.addEventListener('mouseleave', () => {
    const localCursor = singleScreenContainer.querySelector('#single-local-cursor');
    if (localCursor) localCursor.classList.remove('is-active');
  });

  singleScreenContainer.addEventListener('mousedown', () => {
    if (currentFocusedVmId) {
      const client = vmClients.get(currentFocusedVmId);
      if (client?.rfb) {
        try { client.rfb.focus(); } catch (_) {}
      }
    }
  });

  // Render the 24 cards
  function renderFleet() {
    if (fleetList.length === 0) {
      for (const id of vmClients.keys()) {
        disconnectVm(id);
      }
      vmClients.clear();
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

    // Fast in-place status update if cards already match
    const existingCards = gridView.querySelectorAll('.screen-card');
    if (existingCards.length === fleetList.length && vmClients.size === fleetList.length) {
      for (const vm of fleetList) {
        const client = vmClients.get(vm.id);
        if (!client) continue;
        client.vm = vm;
        const isRunning = vm.status === 'running';
        const indicator = client.cardEl.querySelector('.status-indicator');
        if (indicator) {
          indicator.className = `status-indicator ${isRunning ? 'running' : 'stopped'}`;
        }
        const btnPower = client.cardEl.querySelector('.btn-card-power');
        if (btnPower) {
          btnPower.textContent = isRunning ? 'Stop' : 'Start';
        }
        const overlay = client.cardEl.querySelector('.screen-overlay');
        if (!isRunning) {
          disconnectVm(vm.id);
          if (overlay) {
            overlay.classList.remove('hidden');
            overlay.innerHTML = '<p>Computer is stopped</p><button type="button" class="btn-overlay-start primary">Start computer</button>';
          }
        } else if (!client.connected && !client.connecting && currentFocusedVmId !== vm.id) {
          queueConnectVm(vm.id, client.viewportEl);
        }
      }
      return;
    }

    // Full DOM rebuild: disconnect any existing clients cleanly first
    for (const id of vmClients.keys()) {
      disconnectVm(id);
    }
    vmClients.clear();
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
            <button type="button" class="btn-card-delete danger" title="Delete this ChromeOS VM">Delete</button>
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

      const btnCardDelete = card.querySelector('.btn-card-delete');
      if (btnCardDelete) {
        btnCardDelete.addEventListener('click', async e => {
          e.stopPropagation();
          const name = vm.friendlyName || vm.hostname;
          if (!confirm(`Delete ${name}? (Only allowed for VMs with 'chromeos' in the name)`)) {
            return;
          }
          btnCardDelete.disabled = true;
          setGlobalStatus(`Deleting ${name}...`);
          try {
            await api('/api/admin/vms/fleet/delete-chromeos', { id: vm.id });
            disconnectVm(vm.id);
            setGlobalStatus(`${name} deleted.`);
            await loadFleet();
          } catch (err) {
            setGlobalStatus(err.message, true);
            btnCardDelete.disabled = false;
          }
        });
      }

      gridView.appendChild(card);

      vmClients.set(vm.id, {
        vm,
        rfb: null,
        connected: false,
        connecting: false,
        cardEl: card,
        viewportEl,
        jiggleState: false,
        retries: 0
      });

      // If running, queue connection
      if (isRunning && currentFocusedVmId !== vm.id) {
        queueConnectVm(vm.id, viewportEl);
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

  if (btnForceStopAll) {
    btnForceStopAll.addEventListener('click', async () => {
      if (!confirm('Force stop all 24 ChromeOS fleet VMs immediately? (Unsaved data may be lost)')) return;
      btnForceStopAll.disabled = true;
      setGlobalStatus('Force stopping all 24 ChromeOS VMs...');
      try {
        await api('/api/admin/vms/fleet/power', { action: 'force-stop' });
        setGlobalStatus('Force stop command sent to all 24 VMs.');
        for (const id of vmClients.keys()) {
          disconnectVm(id);
        }
        setTimeout(loadFleet, 2500);
      } catch (err) {
        setGlobalStatus(err.message, true);
      } finally {
        btnForceStopAll.disabled = false;
      }
    });
  }

  btnRefresh.addEventListener('click', () => {
    loadFleet();
  });

  btnDeleteAllFleet.addEventListener('click', async () => {
    if (!confirm('Are you sure you want to delete all ChromeOS fleet VMs? This will permanently delete only VMs that have "chromeos" in the name.')) {
      return;
    }
    btnDeleteAllFleet.disabled = true;
    setGlobalStatus('Deleting ChromeOS fleet VMs...');
    try {
      const res = await api('/api/admin/vms/fleet/delete-chromeos', { all: true });
      setGlobalStatus(res.message || 'ChromeOS fleet deleted.');
      for (const id of vmClients.keys()) {
        disconnectVm(id);
      }
      setTimeout(loadFleet, 1500);
    } catch (err) {
      setGlobalStatus(err.message, true);
    } finally {
      btnDeleteAllFleet.disabled = false;
    }
  });

  if (btnDeleteLocalFleet) {
    btnDeleteLocalFleet.addEventListener('click', async () => {
      if (!confirm('Delete ChromeOS fleet from mitch.pro database only? This skips Proxmox and assumes the VMs were manually deleted on the host.')) {
        return;
      }
      btnDeleteLocalFleet.disabled = true;
      setGlobalStatus('Removing ChromeOS fleet from database...');
      try {
        const res = await api('/api/admin/vms/fleet/delete-chromeos', { all: true, localOnly: true });
        setGlobalStatus(res.message || 'Records removed from database.');
        for (const id of vmClients.keys()) {
          disconnectVm(id);
        }
        setTimeout(loadFleet, 1500);
      } catch (err) {
        setGlobalStatus(err.message, true);
      } finally {
        btnDeleteLocalFleet.disabled = false;
      }
    });
  }

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

  btnSingleDelete.addEventListener('click', async () => {
    if (!currentFocusedVmId) return;
    const client = vmClients.get(currentFocusedVmId);
    const name = client?.vm?.friendlyName || client?.vm?.hostname || 'this VM';
    if (!confirm(`Are you sure you want to delete ${name}? (Only VMs with "chromeos" in the name can be deleted)`)) {
      return;
    }
    btnSingleDelete.disabled = true;
    try {
      await api('/api/admin/vms/fleet/delete-chromeos', { id: currentFocusedVmId });
      closeSingleView();
      await loadFleet();
    } catch (err) {
      alert(err.message);
    } finally {
      btnSingleDelete.disabled = false;
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
