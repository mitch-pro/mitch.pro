#!/usr/bin/env bash
set -euo pipefail
# Run only on the Proxmox host against a dedicated, tested template build.
TEMPLATE_ID="${1:?Provide the template build VMID}"
BUILD_USER="${2:?Provide the temporary build username}"
[[ "$TEMPLATE_ID" =~ ^9[0-9]{3}$ ]] || { echo 'Use a dedicated template build between 9000 and 9999.'; exit 1; }
[[ "$BUILD_USER" =~ ^[a-z_][a-z0-9_-]{0,30}$ && "$BUILD_USER" != root ]] || exit 1
qm status "$TEMPLATE_ID" | grep -q running
RESULT="$(qm guest exec "$TEMPLATE_ID" --pass-stdin 1 --timeout 30 -- /bin/bash -s -- "$BUILD_USER" <<'GUEST'
set -euo pipefail
build_user="$1"
[[ $(cat /var/lib/mitch-desktop/setup-version) -ge 2 ]]
gst-inspect-1.0 avdec_h264 >/dev/null
systemctl is-enabled mitch-desktop-login.service >/dev/null
systemctl stop display-manager
loginctl terminate-user "$build_user" || true
for _ in $(seq 1 10); do pgrep -u "$build_user" >/dev/null || break; sleep 1; done
pkill -KILL -u "$build_user" || true
if id "$build_user" >/dev/null 2>&1; then userdel -f -r "$build_user"; fi
rm -f "/etc/sudoers.d/90-mitch-desktop-$build_user"
# Forget the build user in AccountsService and the LightDM greeter/session caches.
rm -f "/var/lib/AccountsService/users/$build_user" "/var/lib/AccountsService/icons/$build_user" \
  "/var/cache/lightdm/dmrc/$build_user.dmrc"
rm -rf /var/lib/lightdm/.cache/slick-greeter /var/lib/lightdm/.cache/lightdm-gtk-greeter
# Each clone's boot service writes autologin for its own cloud-init user.
rm -f /etc/lightdm/lightdm.conf.d/50-autologin.conf
if [[ -f /etc/gdm3/custom.conf ]]; then
python3 - <<'PY'
import configparser, pathlib
p=pathlib.Path('/etc/gdm3/custom.conf')
c=configparser.ConfigParser(strict=False)
c.optionxform=str
c.read(p)
if not c.has_section('daemon'): c.add_section('daemon')
c['daemon']['AutomaticLoginEnable']='false'
c['daemon'].pop('AutomaticLogin',None)
with p.open('w') as f: c.write(f)
PY
fi
passwd -l ubuntu >/dev/null 2>&1 || true
find /home /root -name authorized_keys -type f -delete
# userdel leaves the old entry (with its password hash) in the shadow-utils
# backups; cloud-init rewrites its sudoers file for each clone's own user.
for f in passwd group shadow gshadow subuid subgid; do
  if [[ -e "/etc/$f-" ]]; then cp -p "/etc/$f" "/etc/$f-"; fi
done
rm -f /etc/sudoers.d/90-cloud-init-users
# cupsd rewrites subscriptions (owned by the build user) when it stops.
systemctl stop cups.service cups-browsed.service >/dev/null 2>&1 || true
rm -f /etc/cups/subscriptions.conf /etc/cups/subscriptions.conf.O
journalctl --rotate >/dev/null 2>&1 || true
journalctl --vacuum-time=1s >/dev/null 2>&1 || true
find /var/log -type f \( -name '*.gz' -o -name '*.[0-9]' -o -name '*.old' \) -delete
find /var/log -type f -exec truncate -s 0 {} +
rm -f /etc/ssh/ssh_host_* /root/.bash_history /home/ubuntu/.bash_history /tmp/mitch-video-check.mp4
apt-get clean
cloud-init clean --logs --machine-id --seed
sync
GUEST
)"
jq -e '.exited == 1 and .exitcode == 0' <<<"$RESULT" >/dev/null || { jq '{exitcode,exited,error:."err-data"}' <<<"$RESULT"; exit 1; }
qm shutdown "$TEMPLATE_ID" --timeout 30
for _ in $(seq 1 30); do
  qm status "$TEMPLATE_ID" | grep -q stopped && break
  sleep 1
done
qm status "$TEMPLATE_ID" | grep -q stopped
for key in cipassword sshkeys; do
  if qm config "$TEMPLATE_ID" | grep -q "^$key:"; then qm set "$TEMPLATE_ID" --delete "$key" >/dev/null; fi
done
# Build outside the pool: the portal's uptime watchdog shuts down any running,
# non-template guest it can see (the pool) after 10 minutes without a console.
# The finished template joins the pool so the portal's ACLs apply to it.
if ! pvesh get /pools/sandboxes --output-format json | jq -e --argjson id "$TEMPLATE_ID" 'any(.members[]; .vmid == $id)' >/dev/null; then
  pveum pool modify sandboxes --vms "$TEMPLATE_ID"
fi
qm template "$TEMPLATE_ID"
echo "Sealed desktop template $TEMPLATE_ID. Validate a fresh clone before enabling it."
