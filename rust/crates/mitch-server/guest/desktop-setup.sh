#!/bin/sh
set -eu
user=__DESKTOP_USERNAME__
pass_b64='__DESKTOP_PASSWORD_BASE64__'
timeout 45 cloud-init status --wait >/dev/null 2>&1 || true
# A cloned cloud image may retain interfaces that never become online.
for unit in systemd-networkd-wait-online.service NetworkManager-wait-online.service; do
  systemctl disable --now "$unit" >/dev/null 2>&1 || true
  systemctl mask "$unit" >/dev/null 2>&1 || true
done
systemctl set-default graphical.target >/dev/null
modprobe virtio_rng >/dev/null 2>&1 || true
if ! id "$user" >/dev/null 2>&1; then
  if getent group "$user" >/dev/null 2>&1; then
    useradd -m -s /bin/bash -g "$user" "$user"
  else
    useradd -m -s /bin/bash -U "$user"
  fi
fi
if [ -n "$pass_b64" ]; then
  pass=$(printf '%s' "$pass_b64" | base64 -d)
  printf '%s:%s\n' "$user" "$pass" | chpasswd
fi
usermod -aG sudo,adm "$user" >/dev/null 2>&1 || true
install -d -m 0755 "/home/$user/.config" /var/lib/AccountsService/users
touch "/home/$user/.config/gnome-initial-setup-done"
chown "$user:$(id -gn "$user")" "/home/$user/.config" "/home/$user/.config/gnome-initial-setup-done"
# Desktop access is already authenticated by the website; keep the guest password random.
printf '%s ALL=(ALL) NOPASSWD: ALL\n' "$user" >"/etc/sudoers.d/90-mitch-desktop-$user"
chmod 0440 "/etc/sudoers.d/90-mitch-desktop-$user"
visudo -cf "/etc/sudoers.d/90-mitch-desktop-$user" >/dev/null
if id ubuntu >/dev/null 2>&1 && [ "$user" != ubuntu ]; then
  passwd -l ubuntu >/dev/null 2>&1 || true
  printf '[User]\nSystemAccount=true\n' >/var/lib/AccountsService/users/ubuntu
fi
# Apply session defaults without changing the user's files or restarting an active desktop.
install -d /etc/dconf/db/local.d /etc/dconf/profile
if [ ! -e /etc/dconf/profile/user ]; then
  printf 'user-db:user\nsystem-db:local\n' >/etc/dconf/profile/user
elif ! grep -qx 'system-db:local' /etc/dconf/profile/user; then
  printf '\nsystem-db:local\n' >>/etc/dconf/profile/user
fi
cat >/etc/dconf/db/local.d/20-mitch-desktop <<'EOF'
[org/gnome/desktop/session]
idle-delay=uint32 0
[org/gnome/desktop/screensaver]
lock-enabled=false
[org/gnome/desktop/lockdown]
disable-lock-screen=true
EOF
dconf update
if [ -d /etc/gdm3 ]; then
  # Preserve unrelated GDM settings; only change the managed login keys.
  python3 - "$user" <<'PY'
import configparser, pathlib, sys
p=pathlib.Path('/etc/gdm3/custom.conf')
c=configparser.ConfigParser(strict=False)
c.optionxform=str
c.read(p)
if not c.has_section('daemon'): c.add_section('daemon')
c['daemon']['AutomaticLoginEnable']='true'
c['daemon']['AutomaticLogin']=sys.argv[1]
c['daemon']['WaylandEnable']='false'
with p.open('w') as f: c.write(f)
PY
elif [ -d /etc/lightdm ]; then
  install -d /etc/lightdm/lightdm.conf.d
  printf '[Seat:*]\nautologin-user=%s\nautologin-user-timeout=0\n' "$user" >/etc/lightdm/lightdm.conf.d/50-autologin.conf
else
  echo 'No supported graphical display manager is installed.' >&2
  exit 1
fi
# Read each clone's assigned cloud-init user before the display manager starts.
install -d /usr/local/lib/mitch-desktop
cat >/usr/local/lib/mitch-desktop/autologin.py <<'PYGUEST'
import configparser, pathlib, pwd, re, yaml
p=pathlib.Path('/var/lib/cloud/instance/cloud-config.txt')
if not p.exists(): raise SystemExit(0)
c=yaml.safe_load(p.read_text()) or {}
u=c.get('user')
if not isinstance(u,str) or not re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}',u): raise SystemExit(0)
try: account=pwd.getpwnam(u)
except KeyError: raise SystemExit(0)
if account.pw_uid<1000: raise SystemExit(0)
p=pathlib.Path('/etc/gdm3/custom.conf')
if p.parent.exists():
    cfg=configparser.ConfigParser(strict=False)
    cfg.optionxform=str
    cfg.read(p)
    if not cfg.has_section('daemon'): cfg.add_section('daemon')
    cfg['daemon'].update({'AutomaticLoginEnable':'true','AutomaticLogin':u,'WaylandEnable':'false'})
    with p.open('w') as f: cfg.write(f)
p=pathlib.Path('/etc/lightdm/lightdm.conf.d/50-autologin.conf')
if p.parent.parent.exists():
    p.parent.mkdir(exist_ok=True)
    p.write_text('[Seat:*]\nautologin-user='+u+'\nautologin-user-timeout=0\n')
sudo=pathlib.Path('/etc/sudoers.d/90-mitch-desktop-'+u)
sudo.write_text(u+' ALL=(ALL) NOPASSWD: ALL\n')
sudo.chmod(0o440)
PYGUEST
cat >/etc/systemd/system/mitch-desktop-login.service <<'EOF'
[Unit]
Description=Prepare assigned desktop login
After=cloud-config.service
Before=display-manager.service
[Service]
Type=oneshot
ExecStart=/usr/bin/python3 /usr/local/lib/mitch-desktop/autologin.py
RemainAfterExit=yes
[Install]
WantedBy=graphical.target
EOF
systemctl daemon-reload
systemctl enable mitch-desktop-login.service >/dev/null

uid=$(id -u "$user")
ready() {
  pgrep -u "$uid" -x gnome-shell >/dev/null || pgrep -u "$uid" -x xfce4-session >/dev/null || pgrep -u "$uid" -x cinnamon >/dev/null || pgrep -u "$uid" -x mate-session >/dev/null
}
if ! ready; then
  # Leave any existing human session intact. A logged-out greeter can safely restart.
  if ! loginctl list-sessions --no-legend | awk '$4=="seat0" && $3!="gdm" {found=1} END{exit !found}'; then
    systemctl restart display-manager
  fi
fi
for attempt in $(seq 1 60); do
  if ready; then break; fi
  sleep 1
done
if ! ready; then
  echo 'No graphical session started for the assigned desktop user.' >&2
  exit 1
fi
loginctl unlock-sessions >/dev/null 2>&1 || true
# Install video decoders once. No font EULA or interactive package prompts.
if command -v apt-get >/dev/null; then
  packages='gstreamer1.0-libav gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly ffmpeg'
  missing=''
  for package in $packages; do
    if ! dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -qx 'install ok installed'; then missing="$missing $package"; fi
  done
  if [ -n "$missing" ]; then
    export DEBIAN_FRONTEND=noninteractive
    timeout 90 apt-get -o DPkg::Lock::Timeout=45 update -qq
    timeout 180 apt-get -o DPkg::Lock::Timeout=45 install -y --no-install-recommends $missing >/dev/null
  fi
fi
install -d /var/lib/mitch-desktop
printf '2\n' >/var/lib/mitch-desktop/setup-version
