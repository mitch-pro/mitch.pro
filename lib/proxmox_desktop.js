const DEFAULT_TIMEOUT_MS = 12_000;

export class ProxmoxServiceError extends Error {
  constructor(code, message, status = 502) {
    super(message);
    this.name = 'ProxmoxServiceError';
    this.code = code;
    this.status = status;
  }
}

function parseBoolean(value, fallback) {
  if (value == null || value === '') return fallback;
  return !['0', 'false', 'no', 'off'].includes(String(value).trim().toLowerCase());
}

function normalizedApiBase(host, port) {
  const raw = String(host || '').trim();
  if (!raw) return '';
  if (/^https?:\/\//i.test(raw)) {
    return raw.replace(/\/+$/, '').replace(/\/api2\/json$/i, '') + '/api2/json';
  }
  return `https://${raw.replace(/\/+$/, '')}:${Number(port) || 8006}/api2/json`;
}

function valueFromLegacyUrl(rawUrl) {
  try {
    const url = new URL(rawUrl);
    return { host: url.hostname, port: Number(url.port) || 8006 };
  } catch {
    return { host: '', port: 8006 };
  }
}

function parseDiskGb(config) {
  for (const key of ['scsi0', 'virtio0', 'sata0']) {
    const value = String(config?.[key] || '');
    const match = value.match(/(?:^|,)size=(\d+(?:\.\d+)?)([TGMK])/i);
    if (!match) continue;
    const number = Number(match[1]);
    const unit = match[2].toUpperCase();
    if (unit === 'T') return Math.round(number * 1024);
    if (unit === 'G') return Math.round(number);
    if (unit === 'M') return Math.max(1, Math.round(number / 1024));
    return Math.max(1, Math.round(number / 1024 / 1024));
  }
  return 0;
}

function firstPrivateIpv4(agentResult) {
  const interfaces = agentResult?.result || agentResult || [];
  for (const iface of Array.isArray(interfaces) ? interfaces : []) {
    for (const address of iface?.['ip-addresses'] || []) {
      const value = String(address?.['ip-address'] || '');
      if (address?.['ip-address-type'] === 'ipv4' && value && !value.startsWith('127.') && value !== '0.0.0.0') {
        return value;
      }
    }
  }
  return '';
}

export class ProxmoxDesktopService {
  constructor({ host, port = 8006, node, tokenId, tokenSecret, legacyToken, verifyTls = true, tlsServerName = '', templateVmids = [9010] } = {}) {
    const legacy = valueFromLegacyUrl(host);
    this.host = legacy.host || String(host || '').replace(/^https?:\/\//i, '').split('/')[0].split(':')[0];
    this.port = legacy.host ? legacy.port : (Number(port) || 8006);
    this.node = String(node || '').trim();
    this.baseUrl = normalizedApiBase(this.host, this.port);
    const hasDedicatedConfiguration = Boolean(String(tokenId || '').trim() || String(tokenSecret || '').trim());
    this.authorization = hasDedicatedConfiguration
      ? (tokenId && tokenSecret ? `PVEAPIToken=${String(tokenId).trim()}=${String(tokenSecret).trim()}` : '')
      : String(legacyToken || '').trim();
    this.verifyTls = parseBoolean(verifyTls, true);
    this.tlsServerName = String(tlsServerName || '').trim();
    this.templateVmids = [...new Set((templateVmids || []).map(Number).filter(Number.isInteger))];
    this.guestIpCache = new Map();
    this.guestHostnameCache = new Map();
    this.guestAgentLastAttempt = new Map();
    this.optimizedVmids = new Set();
  }

  static fromEnv(env = process.env) {
    const legacyUrl = String(env.PVE_URL || '').trim();
    const legacy = valueFromLegacyUrl(legacyUrl);
    const templates = String(env.PROXMOX_DESKTOP_TEMPLATES || env.PVE_TEMPLATE_LINUX || '9010')
      .split(',').map(value => Number(value.trim())).filter(Number.isInteger);
    return new ProxmoxDesktopService({
      host: env.PROXMOX_HOST || legacy.host || '192.168.100.1',
      port: env.PROXMOX_PORT || legacy.port || 8006,
      node: env.PROXMOX_NODE || env.PVE_NODE || 'tartarus',
      tokenId: env.PROXMOX_TOKEN_ID,
      tokenSecret: env.PROXMOX_TOKEN_SECRET,
      legacyToken: env.PVE_TOKEN,
      verifyTls: env.PROXMOX_VERIFY_TLS,
      tlsServerName: env.PROXMOX_TLS_SERVERNAME,
      templateVmids: templates,
    });
  }

  get configured() {
    return Boolean(this.baseUrl && this.node && this.authorization);
  }

  get tlsOptions() {
    return { rejectUnauthorized: this.verifyTls, ...(this.tlsServerName ? { serverName: this.tlsServerName } : {}) };
  }

  assertConfigured() {
    if (!this.configured) throw new ProxmoxServiceError('NOT_CONFIGURED', 'Proxmox is not configured.', 503);
  }

  assertVmid(vmid) {
    const value = Number(vmid);
    if (!Number.isInteger(value) || value < 100 || value > 9_999_999) {
      throw new ProxmoxServiceError('INVALID_VM', 'Invalid computer record.', 400);
    }
    return value;
  }

  async request(method, apiPath, params = null, timeoutMs = DEFAULT_TIMEOUT_MS) {
    this.assertConfigured();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const headers = { Authorization: this.authorization };
      const options = {
        method,
        headers,
        signal: controller.signal,
        tls: this.tlsOptions,
        redirect: 'error',
      };
      if (params && method !== 'GET') {
        headers['Content-Type'] = 'application/x-www-form-urlencoded';
        const form = new URLSearchParams();
        for (const [key, value] of Object.entries(params)) {
          if (Array.isArray(value)) value.forEach(item => form.append(key, String(item)));
          else form.append(key, String(value));
        }
        options.body = form.toString();
      }
      const response = await fetch(this.baseUrl + apiPath, options);
      let payload = null;
      try { payload = await response.json(); } catch {}
      if (!response.ok) {
        const status = response.status === 401 || response.status === 403 ? 503 : response.status;
        throw new ProxmoxServiceError('UPSTREAM_REJECTED', 'The computer service rejected the request.', status);
      }
      return payload?.data;
    } catch (error) {
      if (error instanceof ProxmoxServiceError) throw error;
      if (error?.name === 'AbortError') throw new ProxmoxServiceError('TIMEOUT', 'The computer service timed out.', 504);
      throw new ProxmoxServiceError('UNREACHABLE', 'The computer service is unreachable.', 502);
    } finally {
      clearTimeout(timer);
    }
  }

  async listGuests() {
    const rows = await this.request('GET', '/cluster/resources?type=vm');
    return (Array.isArray(rows) ? rows : []).map(row => ({
      vmid: Number(row.vmid), node: String(row.node || this.node), type: String(row.type || 'qemu'),
      name: String(row.name || `computer-${row.vmid}`), status: String(row.status || 'unknown'),
      template: Boolean(row.template), cpuCores: Number(row.maxcpu || 0), memoryMb: Math.round(Number(row.maxmem || 0) / 1024 / 1024),
      diskGb: Math.round(Number(row.maxdisk || 0) / 1024 / 1024 / 1024),
      uptime: Number(row.uptime || 0),
    }));
  }

  async nodeCapacity() {
    const row = await this.request('GET', `/nodes/${encodeURIComponent(this.node)}/status`);
    const memory = row?.memory || {};
    const rootfs = row?.rootfs || {};
    return {
      cpuUsage: Number(row?.cpu || 0), cpuCores: Number(row?.cpuinfo?.cpus || row?.cpuinfo?.cores || 0),
      memoryUsed: Number(memory.used || 0), memoryTotal: Number(memory.total || 0),
      storageUsed: Number(rootfs.used || 0), storageTotal: Number(rootfs.total || 0),
      uptime: Number(row?.uptime || 0),
    };
  }

  async getConfig(record) {
    const vmid = this.assertVmid(record.vmid);
    return await this.request('GET', `/nodes/${encodeURIComponent(record.node || this.node)}/${record.guestType || 'qemu'}/${vmid}/config`);
  }

  async getStatus(record) {
    const vmid = this.assertVmid(record.vmid);
    const node = encodeURIComponent(record.node || this.node);
    const type = record.guestType || 'qemu';
    const status = await this.request('GET', `/nodes/${node}/${type}/${vmid}/status/current`);
    if (status?.status !== 'running') {
      this.guestIpCache?.delete(vmid);
      this.guestHostnameCache?.delete(vmid);
      this.guestAgentLastAttempt?.delete(vmid);
    }
    let ipAddress = record.ipAddress || this.guestIpCache?.get(vmid) || '';
    let hostname = record.hostname || this.guestHostnameCache?.get(vmid) || '';
    const lastAttempt = this.guestAgentLastAttempt?.get(vmid) || 0;
    const shouldQueryAgent = type === 'qemu' && status?.status === 'running' && (!ipAddress || !hostname) && (Date.now() - lastAttempt > 300_000);
    if (shouldQueryAgent) {
      this.guestAgentLastAttempt?.set(vmid, Date.now());
      if (!ipAddress) {
        try {
          const agent = await this.request('GET', `/nodes/${node}/qemu/${vmid}/agent/network-get-interfaces`, null, 1500);
          ipAddress = firstPrivateIpv4(agent) || ipAddress;
          if (ipAddress) this.guestIpCache?.set(vmid, ipAddress);
        } catch {}
      }
      if (!hostname) {
        try {
          const agentHost = await this.request('GET', `/nodes/${node}/qemu/${vmid}/agent/get-host-name`, null, 1000);
          hostname = String(agentHost?.result?.['host-name'] || agentHost?.['host-name'] || hostname);
          if (hostname) this.guestHostnameCache?.set(vmid, hostname);
        } catch {}
      }
    }
    return {
      state: String(status?.status || 'unknown'), cpuUsage: Number(status?.cpu || 0),
      cpuCores: Number(status?.cpus || record.cpuCores || 0), memoryUsed: Number(status?.mem || 0),
      memoryTotal: Number(status?.maxmem || (record.memoryMb || 0) * 1024 * 1024),
      diskUsed: Number(status?.disk || 0), diskTotal: Number(status?.maxdisk || (record.diskGb || 0) * 1024 * 1024 * 1024),
      uptime: Number(status?.uptime || 0), ipAddress, hostname,
    };
  }

  async ensureOptimizedVmConfig(vmid, node = this.node) {
    vmid = this.assertVmid(vmid);
    if (this.optimizedVmids?.has(vmid)) return null;
    try {
      const config = await this.request('GET', `/nodes/${encodeURIComponent(node)}/qemu/${vmid}/config`);
      const updates = {};
      if (config?.cpu !== 'host') updates.cpu = 'host';
      if (!config?.rng0) updates.rng0 = 'source=/dev/urandom';
      if (!config?.vga || config.vga === 'virtio' || config.vga === 'std') {
        updates.vga = 'std,memory=64';
      }
      if (Object.keys(updates).length > 0) {
        await this.request('PUT', `/nodes/${encodeURIComponent(node)}/qemu/${vmid}/config`, updates);
        console.log(`[proxmox] Applied boot optimizations to VM ${vmid}:`, Object.keys(updates).join(', '));
      }
      this.optimizedVmids?.add(vmid);
      return updates;
    } catch (err) {
      console.warn(`[proxmox] Could not optimize VM ${vmid} config:`, err?.message || err);
      return null;
    }
  }

  async ensureGuestOptimized(vmid, node = this.node) {
    vmid = this.assertVmid(vmid);
    try {
      await this.request('POST', `/nodes/${encodeURIComponent(node)}/qemu/${vmid}/agent/ping`, {}, 2000);
    } catch {
      return;
    }
    const script = `set -eu
systemctl disable --now systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
systemctl mask systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
modprobe virtio_rng >/dev/null 2>&1 || true
if [ -f /etc/gdm3/custom.conf ]; then
  if grep -q '^#*WaylandEnable=' /etc/gdm3/custom.conf; then
    sed -i 's/^#*WaylandEnable=.*/WaylandEnable=false/' /etc/gdm3/custom.conf
  elif grep -q '\\[daemon\\]' /etc/gdm3/custom.conf; then
    sed -i '/\\[daemon\\]/a WaylandEnable=false' /etc/gdm3/custom.conf
  fi
  if ! pgrep -x Xorg >/dev/null 2>&1; then
    systemctl restart gdm3 >/dev/null 2>&1 || true
  fi
fi
exit 0
`;
    try {
      await this.guestExec(vmid, ['/bin/sh', '-s'], script, 15_000);
    } catch (e) {
      console.warn(`[proxmox] Background guest optimization skipped for VM ${vmid}:`, e?.message || e);
    }
  }

  async power(record, requestedAction) {
    const vmid = this.assertVmid(record.vmid);
    const actions = { start: 'start', shutdown: 'shutdown', restart: 'reboot', 'force-stop': 'stop' };
    const action = actions[requestedAction];
    if (!action) throw new ProxmoxServiceError('INVALID_ACTION', 'Invalid power action.', 400);
    if ((record.guestType || 'qemu') === 'qemu' && (requestedAction === 'start' || requestedAction === 'restart')) {
      await this.ensureOptimizedVmConfig(vmid, record.node || this.node).catch(() => {});
      setTimeout(() => {
        void this.ensureGuestOptimized(vmid, record.node || this.node).catch(() => {});
      }, 5000);
    }
    return await this.request('POST', `/nodes/${encodeURIComponent(record.node || this.node)}/${record.guestType || 'qemu'}/${vmid}/status/${action}`, {});
  }

  async deleteGuest(record, { force = false } = {}) {
    const vmid = this.assertVmid(record.vmid);
    const node = encodeURIComponent(record.node || this.node);
    const type = record.guestType || 'qemu';
    try {
      try {
        await this.power(record, 'force-stop');
        await Bun.sleep(1000);
      } catch (_) {}
      const upid = await this.request('DELETE', `/nodes/${node}/${type}/${vmid}?purge=1&destroy-unreferenced-disks=1`, null, 30_000);
      if (upid) await this.waitForTask(record.node || this.node, upid, 30_000);
      return { success: true };
    } catch (error) {
      if (force) {
        return { success: true, ignoredError: error?.message || String(error) };
      }
      throw error;
    }
  }

  async createConsole(record) {
    const vmid = this.assertVmid(record.vmid);
    if ((record.guestType || 'qemu') !== 'qemu') {
      throw new ProxmoxServiceError('NO_GRAPHICAL_DESKTOP', 'This computer does not have a graphical desktop.', 409);
    }
    const status = await this.getStatus(record);
    if (status.state !== 'running') throw new ProxmoxServiceError('STOPPED', 'The computer is not running.', 409);
    const node = record.node || this.node;
    void this.ensureOptimizedVmConfig(vmid, node).catch(() => {});
    void this.ensureGuestOptimized(vmid, node).catch(() => {});
    const consoleData = await this.request('POST', `/nodes/${encodeURIComponent(node)}/qemu/${vmid}/vncproxy`, { websocket: 1 });
    const port = Number(consoleData?.port);
    const ticket = String(consoleData?.ticket || '');
    if (!Number.isInteger(port) || port < 5900 || port > 5999 || !ticket) {
      throw new ProxmoxServiceError('CONSOLE_FAILED', 'The desktop connection could not be created.', 502);
    }
    const wsBase = this.baseUrl.replace(/^http/i, 'ws').replace(/\/api2\/json$/, '');
    const wsUrl = `${wsBase}/api2/json/nodes/${encodeURIComponent(node)}/qemu/${vmid}/vncwebsocket?port=${port}&vncticket=${encodeURIComponent(ticket)}`;
    return { wsUrl, port, ticket, authorization: this.authorization, tlsOptions: this.tlsOptions };
  }

  async waitForTask(node, upid, timeoutMs = 120_000) {
    if (!upid) return;
    const started = Date.now();
    while (Date.now() - started < timeoutMs) {
      const status = await this.request('GET', `/nodes/${encodeURIComponent(node)}/tasks/${encodeURIComponent(upid)}/status`, null, 8000);
      if (status?.status === 'stopped') {
        if (status.exitstatus && status.exitstatus !== 'OK') throw new ProxmoxServiceError('TASK_FAILED', 'The computer could not be prepared.', 502);
        return;
      }
      await Bun.sleep(1000);
    }
    throw new ProxmoxServiceError('TASK_TIMEOUT', 'The computer is still being prepared.', 504);
  }

  async waitForGuestAgent(vmid, timeoutMs = 180_000) {
    vmid = this.assertVmid(vmid);
    const started = Date.now();
    while (Date.now() - started < timeoutMs) {
      try {
        await this.request('POST', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/agent/ping`, {}, 5000);
        return;
      } catch {}
      await Bun.sleep(2000);
    }
    throw new ProxmoxServiceError('GUEST_SETUP_FAILED', 'The graphical desktop did not finish starting.', 504);
  }

  async guestExec(vmid, command, inputData = '', timeoutMs = 150_000) {
    vmid = this.assertVmid(vmid);
    const started = await this.request('POST', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/agent/exec`, {
      command,
      ...(inputData ? { 'input-data': inputData } : {}),
    });
    const pid = Number(started?.pid);
    if (!Number.isInteger(pid) || pid < 1) throw new ProxmoxServiceError('GUEST_SETUP_FAILED', 'The graphical desktop could not be prepared.', 502);
    const began = Date.now();
    while (Date.now() - began < timeoutMs) {
      const status = await this.request('GET', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/agent/exec-status?pid=${pid}`, null, 8000);
      if (status?.exited) {
        if (Number(status.exitcode || 0) !== 0) throw new ProxmoxServiceError('GUEST_SETUP_FAILED', 'The graphical desktop could not be prepared.', 502);
        return;
      }
      await Bun.sleep(1000);
    }
    throw new ProxmoxServiceError('GUEST_SETUP_FAILED', 'The graphical desktop is still being prepared.', 504);
  }

  async enableFriendlyDesktopLogin(vmid, username, password = '') {
    const login = this.validateDesktopLogin(username, password || 'temporary-validation-only');
    await this.waitForGuestAgent(vmid);
    const passBase64 = password ? Buffer.from(login.password, 'utf8').toString('base64') : '';
    const script = `set -eu
timeout 15 cloud-init status --wait >/dev/null 2>&1 || true
systemctl disable --now systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
systemctl mask systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
modprobe virtio_rng >/dev/null 2>&1 || true
user=${login.username}
pass_b64="${passBase64}"
if ! id "$user" >/dev/null 2>&1; then
  if getent group "$user" >/dev/null 2>&1; then
    useradd -m -s /bin/bash -g "$user" -G sudo,adm,cdrom,dip "$user"
  else
    useradd -m -s /bin/bash -U -G sudo,adm,cdrom,dip "$user"
  fi
fi
if [ -n "$pass_b64" ]; then
  pass=$(printf '%s' "$pass_b64" | base64 -d)
  printf '%s:%s\\n' "$user" "$pass" | chpasswd
fi
usermod -aG sudo,adm,cdrom,dip "$user" >/dev/null 2>&1 || true
install -d -m 0755 "/home/$user/.config" /var/lib/AccountsService/users
touch "/home/$user/.config/gnome-initial-setup-done"
chown -R "$user:$user" "/home/$user/.config"
sudo -u "$user" dbus-run-session gsettings set org.gnome.desktop.session idle-delay 0 >/dev/null 2>&1 || true
sudo -u "$user" dbus-run-session gsettings set org.gnome.desktop.screensaver lock-enabled false >/dev/null 2>&1 || true
if id ubuntu >/dev/null 2>&1 && [ "$user" != ubuntu ]; then
  passwd -l ubuntu >/dev/null 2>&1 || true
  printf '[User]\\nSystemAccount=true\\n' >/var/lib/AccountsService/users/ubuntu
fi
if [ -d /etc/gdm3 ] || [ -f /etc/gdm3/custom.conf ]; then
  mkdir -p /etc/gdm3
  cat >/etc/gdm3/custom.conf <<EOF
[daemon]
AutomaticLoginEnable=true
AutomaticLogin=$user
WaylandEnable=false

[security]
[xdmcp]
[chooser]
[debug]
EOF
  systemctl restart gdm3 >/dev/null 2>&1 || true
fi
if [ -d /etc/lightdm ]; then
  mkdir -p /etc/lightdm/lightdm.conf.d
  cat >/etc/lightdm/lightdm.conf.d/50-autologin.conf <<EOF
[Seat:*]
autologin-user=$user
autologin-user-timeout=0
EOF
  systemctl restart lightdm >/dev/null 2>&1 || true
fi
uid=$(id -u "$user")
for attempt in $(seq 1 45); do
  if pgrep -u "$uid" -x gnome-shell >/dev/null 2>&1 || pgrep -u "$uid" -x xfce4-session >/dev/null 2>&1 || pgrep -u "$uid" -x cinnamon >/dev/null 2>&1 || pgrep -u "$uid" -x mate-session >/dev/null 2>&1; then
    loginctl unlock-sessions >/dev/null 2>&1 || true
    exit 0
  fi
  sleep 1
done
loginctl unlock-sessions >/dev/null 2>&1 || true
exit 0
`;
    await this.guestExec(vmid, ['/bin/sh', '-s'], script);
  }

  async nextAvailableVmid(min = 200, max = 999, reservedVmids = []) {
    const used = new Set([...(await this.listGuests()).map(vm => vm.vmid), ...reservedVmids.map(Number)]);
    for (let vmid = Number(min); vmid <= Number(max); vmid++) if (!used.has(vmid)) return vmid;
    throw new ProxmoxServiceError('NO_CAPACITY', 'No computer slots are currently available.', 409);
  }

  async cloneDesktop({ templateVmid, vmid, hostname, cpuCores = 2, memoryMb = 4096, diskGb = 64, desktopUsername, desktopPassword }) {
    templateVmid = this.assertVmid(templateVmid);
    vmid = this.assertVmid(vmid);
    if (!this.templateVmids.includes(templateVmid)) throw new ProxmoxServiceError('INVALID_TEMPLATE', 'That desktop template is not allowed.', 400);
    const login = this.validateDesktopLogin(desktopUsername, desktopPassword);
    const templateConfig = await this.getConfig({ vmid: templateVmid, node: this.node, guestType: 'qemu' });
    if (!Number(templateConfig?.template) || !Object.entries(templateConfig).some(([key, value]) => /^(ide|scsi|sata)\d+$/.test(key) && /cloudinit(?:,|$)/.test(String(value)))) {
      throw new ProxmoxServiceError('INVALID_TEMPLATE', 'This template is not ready for automatic desktop setup.', 400);
    }
    if (!templateConfig.net0 || !/(?:^|,)bridge=/.test(String(templateConfig.net0))) {
      throw new ProxmoxServiceError('INVALID_TEMPLATE', 'This template does not have a configured network.', 400);
    }
    cpuCores = Math.max(2, Math.min(Math.round(Number(cpuCores) || 2), 16));
    memoryMb = Math.max(2048, Math.min(Math.round(Number(memoryMb) || 4096), 65536));
    const cleanHostname = String(hostname || `computer-${vmid}`).toLowerCase().replace(/[^a-z0-9-]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '').slice(0, 48) || `computer-${vmid}`;
    const cloneUpid = await this.request('POST', `/nodes/${encodeURIComponent(this.node)}/qemu/${templateVmid}/clone`, {
      newid: vmid, name: cleanHostname, full: 0, pool: 'sandboxes', target: this.node,
    }, 30_000);
    await this.waitForTask(this.node, cloneUpid);
    await this.request('PUT', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/config`, {
      name: cleanHostname, cores: cpuCores, sockets: 1, memory: memoryMb,
      cpu: 'host',
      rng0: 'source=/dev/urandom',
      vga: templateConfig.vga || 'std,memory=64',
      balloon: Math.min(4096, Math.max(1024, Math.floor(memoryMb / 4))), agent: 1,
      ciuser: login.username, cipassword: login.password, ciupgrade: 0, ipconfig0: 'ip=dhcp',
      ...(templateConfig.sshkeys ? { delete: 'sshkeys' } : {}),
    });
    this.optimizedVmids?.add(vmid);
    const config = await this.request('GET', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/config`);
    const currentDiskGb = parseDiskGb(config);
    const requestedDiskGb = Math.max(40, Math.min(Math.round(Number(diskGb) || 64), 256));
    if (currentDiskGb && requestedDiskGb > currentDiskGb) {
      const disk = ['scsi0', 'virtio0', 'sata0'].find(key => config[key]);
      const resizeUpid = await this.request('PUT', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/resize`, { disk, size: `${requestedDiskGb}G` });
      if (resizeUpid) await this.waitForTask(this.node, resizeUpid);
    }
    const startUpid = await this.request('POST', `/nodes/${encodeURIComponent(this.node)}/qemu/${vmid}/status/start`, {});
    if (startUpid) await this.waitForTask(this.node, startUpid);
    void this.enableFriendlyDesktopLogin(vmid, login.username, login.password).then(() => {
      console.log(`[proxmox] Friendly desktop auto-login setup completed for VM ${vmid}`);
    }).catch(err => {
      console.warn(`[proxmox] Friendly desktop auto-login setup deferred or timed out for VM ${vmid}:`, err?.message || err);
    });
    return { vmid, node: this.node, hostname: cleanHostname, cpuCores, memoryMb, diskGb: Math.max(currentDiskGb, requestedDiskGb) };
  }

  async updateHardware(record, { cpuCores, memoryMb, diskGb } = {}) {
    const vmid = this.assertVmid(record.vmid);
    const node = encodeURIComponent(record.node || this.node);
    const updates = {};
    if (cpuCores != null) {
      updates.cores = Math.max(2, Math.min(Math.round(Number(cpuCores)), 16));
    }
    if (memoryMb != null) {
      const mem = Math.max(2048, Math.min(Math.round(Number(memoryMb)), 65536));
      updates.memory = mem;
      updates.balloon = Math.min(4096, Math.max(1024, Math.floor(mem / 4)));
    }
    if (Object.keys(updates).length > 0) {
      await this.request('PUT', `/nodes/${node}/qemu/${vmid}/config`, updates);
    }
    if (diskGb != null) {
      const config = await this.getConfig(record);
      const currentDiskGb = parseDiskGb(config);
      const requestedDiskGb = Math.max(40, Math.min(Math.round(Number(diskGb)), 256));
      if (currentDiskGb && requestedDiskGb > currentDiskGb) {
        const disk = ['scsi0', 'virtio0', 'sata0'].find(key => config[key]);
        if (disk) {
          const resizeUpid = await this.request('PUT', `/nodes/${node}/qemu/${vmid}/resize`, { disk, size: `${requestedDiskGb}G` });
          if (resizeUpid) await this.waitForTask(record.node || this.node, resizeUpid);
        }
      }
    }
    return { success: true };
  }

  validateDesktopLogin(username, password) {
    const cleanUsername = String(username || '').trim().toLowerCase();
    const cleanPassword = String(password || '');
    if (!/^[a-z][a-z0-9_\-]{1,31}$/.test(cleanUsername) || ['root', 'daemon', 'nobody', 'ubuntu'].includes(cleanUsername) || cleanPassword.length < 1 || /[\r\n\0]/.test(cleanPassword)) {
      throw new ProxmoxServiceError('INVALID_DESKTOP_LOGIN', 'Choose a desktop username and a non-empty password without line breaks.', 400);
    }
    return { username: cleanUsername, password: cleanPassword };
  }
}
