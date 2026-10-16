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
[[ $(cat /var/lib/mitch-desktop/setup-version) == 2 ]]
gst-inspect-1.0 avdec_h264 >/dev/null
systemctl is-enabled mitch-desktop-login.service >/dev/null
systemctl stop display-manager
loginctl terminate-user "$build_user" || true
for _ in $(seq 1 10); do pgrep -u "$build_user" >/dev/null || break; sleep 1; done
pkill -KILL -u "$build_user" || true
if id "$build_user" >/dev/null 2>&1; then userdel -f -r "$build_user"; fi
rm -f "/etc/sudoers.d/90-mitch-desktop-$build_user"
python3 - <<'PY'
import configparser, pathlib
p=pathlib.Path('/etc/gdm3/custom.conf')
c=configparser.ConfigParser(strict=False)
c.optionxform=str
c.read(p)
c['daemon']['AutomaticLoginEnable']='false'
c['daemon'].pop('AutomaticLogin',None)
with p.open('w') as f: c.write(f)
PY
passwd -l ubuntu >/dev/null 2>&1 || true
find /home /root -name authorized_keys -type f -delete
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
qm template "$TEMPLATE_ID"
echo "Sealed desktop template $TEMPLATE_ID. Validate a fresh clone before enabling it."
