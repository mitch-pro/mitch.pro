#!/usr/bin/env bash
# Runs the real tools/deploy.sh against stand-in docker/curl/flock/runuser commands and a
# throwaway git repo, to check the health-check retry and the git rollback:
#   never healthy  -> retries once, stops the new slot, rolls git back, exits 1
#   healthy on retry -> keeps the new commit, no rollback, exits 0
#   healthy at once  -> keeps the new commit, no retry, exits 0
# Nothing here touches Docker or the network. Run: bash tests/test_deploy_rollback.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/home"

fail() { echo "FAIL: $*" >&2; [ -f "$WORK/calls" ] && sed 's/^/  call: /' "$WORK/calls" >&2; exit 1; }

# Stand-ins. HEALTH_MODE: never | retry | ok. In `retry` mode the new slot turns healthy
# only after `docker compose up -d webserver-green` (the retry) has been called.
cat > "$WORK/bin/docker" <<STUB
#!/usr/bin/env bash
echo "docker \$*" >> "$WORK/calls"
case "\$1 \$2" in
    "exec mitch-caddy-proxy")
        case "\$*" in *wget*) echo "webserver-blue:6800" ;; esac ;;
    "exec mitch-webserver-blue")
        case "\$HEALTH_MODE" in
            ok) echo 200 ;;
            retry) if [ -f "$WORK/retried" ]; then echo 200; else echo 000; fi ;;
            *) echo 000 ;;
        esac ;;
    "compose --progress=plain") ;;
    "compose up")
        case "\$*" in *"up -d webserver-green") touch "$WORK/retried" ;; esac ;;
esac
exit 0
STUB
cat > "$WORK/bin/curl" <<STUB
#!/usr/bin/env bash
echo "curl \$*" >> "$WORK/calls"
echo 200
STUB
printf '#!/usr/bin/env bash\nexit 0\n' > "$WORK/bin/flock"
# deploy.sh drops to the repo owner with runuser when run as root; just run the command.
printf '#!/usr/bin/env bash\nshift 3\nexec "$@"\n' > "$WORK/bin/runuser"
chmod +x "$WORK/bin"/*
export PATH="$WORK/bin:$PATH" HOME="$WORK/home" GIT_TERMINAL_PROMPT=0

G="git -c user.email=t@t -c user.name=t"
$G init -q --bare "$WORK/origin.git"

# fresh_checkout: a clone one commit behind origin/master, with deploy.sh beside the
# compose file (that is how deploy.sh finds PROJECT_DIR).
fresh_checkout() {
    rm -rf "$WORK/proj" "$WORK/pusher" "$WORK/origin.git"
    $G init -q --bare -b master "$WORK/origin.git"
    $G clone -q "$WORK/origin.git" "$WORK/pusher" 2>/dev/null
    mkdir -p "$WORK/pusher/caddy" "$WORK/pusher/rust"
    : > "$WORK/pusher/docker-compose.yml"
    echo "reverse_proxy webserver-blue:6800" > "$WORK/pusher/caddy/Caddyfile"
    echo old > "$WORK/pusher/rust/app.rs"
    $G -C "$WORK/pusher" add -A
    $G -C "$WORK/pusher" commit -qm old
    $G -C "$WORK/pusher" push -q origin HEAD:master
    $G clone -q "$WORK/origin.git" "$WORK/proj" 2>/dev/null
    cp "$ROOT/tools/deploy.sh" "$WORK/proj/deploy.sh"
    OLD="$($G -C "$WORK/proj" rev-parse HEAD)"
    echo new > "$WORK/pusher/rust/app.rs"     # not a static-only change: forces the full swap
    $G -C "$WORK/pusher" commit -qam new
    $G -C "$WORK/pusher" push -q origin HEAD:master
    NEW="$($G -C "$WORK/pusher" rev-parse HEAD)"
    : > "$WORK/calls"; rm -f "$WORK/retried"
}

run_deploy() {
    HEALTH_MODE="$1" HEALTH_WINDOW_SECS=3 bash "$WORK/proj/deploy.sh" > "$WORK/out" 2>&1 && RC=0 || RC=$?
}
head_of() { $G -C "$WORK/proj" rev-parse HEAD; }

# 1. Never healthy: retry once, stop the failed slot, roll back, fail.
fresh_checkout; run_deploy never
[ "$RC" = 1 ] || { cat "$WORK/out" >&2; fail "never-healthy: expected exit 1, got $RC"; }
[ "$(head_of)" = "$OLD" ] || fail "never-healthy: checkout should be back on the old commit"
grep -qx "docker compose up -d webserver-green" "$WORK/calls" || fail "never-healthy: no retry of the slot"
grep -qx "docker stop mitch-webserver-green" "$WORK/calls" || fail "never-healthy: failed slot not stopped"
grep -q "Rolled back to" "$WORK/out" || fail "never-healthy: rollback not reported"
grep -q "api/cache/refresh" "$WORK/calls" || fail "never-healthy: static cache not refreshed after rollback"
grep -q "webserver-blue:6800" "$WORK/proj/caddy/Caddyfile" || fail "never-healthy: Caddyfile no longer points at the live slot"
! grep -qx "docker stop mitch-webserver-blue" "$WORK/calls" || fail "never-healthy: the live slot must not be stopped"
echo "ok: never healthy -> retried, rolled back, exit 1"

# 2. Healthy only after the retry: no rollback.
fresh_checkout; run_deploy retry
[ "$RC" = 0 ] || { cat "$WORK/out" >&2; fail "retry: expected exit 0, got $RC"; }
[ "$(head_of)" = "$NEW" ] || fail "retry: checkout should stay on the new commit"
grep -qx "docker compose up -d webserver-green" "$WORK/calls" || fail "retry: no retry happened"
! grep -q "Rolling git back" "$WORK/out" || fail "retry: must not roll back"
echo "ok: healthy on retry -> deployed"

# 3. Healthy immediately: no retry, no rollback.
fresh_checkout; run_deploy ok
[ "$RC" = 0 ] || { cat "$WORK/out" >&2; fail "ok: expected exit 0, got $RC"; }
[ "$(head_of)" = "$NEW" ] || fail "ok: checkout should stay on the new commit"
! grep -qx "docker compose up -d webserver-green" "$WORK/calls" || fail "ok: must not retry"
echo "ok: healthy at once -> deployed"
