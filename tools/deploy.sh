#!/usr/bin/env bash
# deploy.sh - Automated Zero-Downtime Blue-Green Swap Deployment Engine

set -euo pipefail

# GitHub pushes can arrive close together. Only one process may choose and
# replace a blue/green slot at a time, otherwise both runs can target the same
# inactive container and interrupt a healthy release.
touch /tmp/mitch-pro-deploy.lock 2>/dev/null || true
chmod 666 /tmp/mitch-pro-deploy.lock 2>/dev/null || true
exec 9>/tmp/mitch-pro-deploy.lock
echo "[deploy] Waiting for the deployment lock..."
flock -w 900 9 || { echo '[deploy] Timed out waiting for another deployment to finish.'; exit 1; }

# Determine project directory
REAL_SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}" 2>/dev/null || echo "${BASH_SOURCE[0]}")"
SCRIPT_DIR="$(cd "$(dirname "$REAL_SCRIPT_PATH")" && pwd)"

if [ -d "/home/mitch/server/bun" ]; then
    PROJECT_DIR="/home/mitch/server/bun"
elif [ -d "/home/mitch/bun-server-main/bun-server" ]; then
    PROJECT_DIR="/home/mitch/bun-server-main/bun-server"
elif [ -f "$SCRIPT_DIR/server.js" ]; then
    PROJECT_DIR="$SCRIPT_DIR"
elif [ -f "$SCRIPT_DIR/../server.js" ]; then
    PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
else
    PROJECT_DIR="$(pwd)"
fi
cd "$PROJECT_DIR"
CADDYFILE_PATH="$PROJECT_DIR/caddy/Caddyfile"

# Determine repo owner to drop privileges cleanly when running as root without sudo
REPO_OWNER="$(stat -c '%U' "$PROJECT_DIR" 2>/dev/null || echo "${SUDO_USER:-mitch}")"
[ -z "$REPO_OWNER" ] || [ "$REPO_OWNER" = "root" ] && REPO_OWNER="${SUDO_USER:-mitch}"

# Helper to run git cleanly without sudo
run_git() {
    if [ "$(id -u)" -eq 0 ]; then
        git config --global --add safe.directory "$PROJECT_DIR" 2>/dev/null || true
        if [ "$REPO_OWNER" != "root" ] && command -v runuser &>/dev/null; then
            runuser -u "$REPO_OWNER" -- git -C "$PROJECT_DIR" "$@"
        elif [ "$REPO_OWNER" != "root" ] && command -v su &>/dev/null; then
            su -s /bin/bash "$REPO_OWNER" -c "git -C \"$PROJECT_DIR\" $(printf '%q ' "$@")"
        else
            git -C "$PROJECT_DIR" "$@"
        fi
    else
        git -C "$PROJECT_DIR" "$@"
    fi
}

# 1. Fetch only NTFY_TOPIC for the deploy script's notifications
NTFY_TOPIC=""
DOPPLER_AVAILABLE=false
export DOPPLER_ENABLE_DNS_RESOLVER=true
if command -v doppler &> /dev/null && doppler secrets download --format json &> /dev/null; then
    DOPPLER_AVAILABLE=true
    NTFY_TOPIC=$(doppler secrets get NTFY_TOPIC --plain 2>/dev/null || echo "")
else
    ENV_PATH="$PROJECT_DIR/.env"
    if [ -f "$ENV_PATH" ]; then
        NTFY_TOPIC=$(grep -E "^NTFY_TOPIC=" "$ENV_PATH" | cut -d= -f2- | tr -d '"' | tr -d "'")
    fi
fi
NTFY_TOPIC="${NTFY_TOPIC:-}"

send_notification() {
    [ -z "${NTFY_TOPIC:-}" ] && return 0
    local msg="$1"
    local title="${2:-Deploy Status}"
    local priority="${3:-default}"
    local url
    if [[ "$NTFY_TOPIC" =~ ^https?:// ]]; then
        url="$NTFY_TOPIC"
    else
        url="https://ntfy.sh/${NTFY_TOPIC#/}"
    fi
    curl -s -H "Title: $title" -H "Priority: $priority" -d "$msg" "$url" > /dev/null || true
}

# Helper to run docker compose wrapped in doppler run (if Doppler is available), keeping secrets off disk and avoiding bash evaluation bugs.
run_docker_compose() {
    if [ "$DOPPLER_AVAILABLE" = true ]; then
        doppler run -- docker compose "$@"
    else
        docker compose "$@"
    fi
}

# Helper: return 0 if all changed files are static webroot assets or doc files
is_only_static() {
    local files="$1"
    [ -z "$files" ] && return 1
    while IFS= read -r file; do
        [ -z "$file" ] && continue
        case "$file" in
            webserver/*|docs/*|*.md|.gitignore|LICENSE|*.txt)
                ;;
            *)
                return 1
                ;;
        esac
    done <<< "$files"
    return 0
}

# 1. Determine which slot is currently active BEFORE touching git or files.
# Check Caddyfile routing first (the true proxy source of truth).
if grep -q "webserver-blue:6800" "$CADDYFILE_PATH" 2>/dev/null; then
    ACTIVE_SLOT="blue"
    INACTIVE_SLOT="green"
    INACTIVE_PORT=6812
elif grep -q "webserver-green:6800" "$CADDYFILE_PATH" 2>/dev/null; then
    ACTIVE_SLOT="green"
    INACTIVE_SLOT="blue"
    INACTIVE_PORT=6811
elif docker ps --filter "name=mitch-webserver-green" --filter "status=running" --format '{{.Names}}' 2>/dev/null | grep -q "mitch-webserver-green"; then
    ACTIVE_SLOT="green"
    INACTIVE_SLOT="blue"
    INACTIVE_PORT=6811
elif docker ps --filter "name=mitch-webserver-blue" --filter "status=running" --format '{{.Names}}' 2>/dev/null | grep -q "mitch-webserver-blue"; then
    ACTIVE_SLOT="blue"
    INACTIVE_SLOT="green"
    INACTIVE_PORT=6812
else
    ACTIVE_SLOT="blue"
    INACTIVE_SLOT="green"
    INACTIVE_PORT=6812
fi

echo "[deploy] Active slot detected: webserver-$ACTIVE_SLOT"
echo "[deploy] Target inactive slot to boot: webserver-$INACTIVE_SLOT (Port $INACTIVE_PORT)"

# 2. Pull the latest code
# Discard local runtime modifications to tracked files (like caddy/Caddyfile) so git pull never fails
echo "[deploy] Ensuring working directory is clean of runtime changes..."
if [ "$(id -u)" -eq 0 ] && [ "$REPO_OWNER" != "root" ]; then
    chown -R "$REPO_OWNER:$REPO_OWNER" "$CADDYFILE_PATH" "$PROJECT_DIR/.git" 2>/dev/null || true
fi
OLD_COMMIT=$(run_git rev-parse HEAD 2>/dev/null || echo "")
run_git checkout -- caddy/Caddyfile 2>/dev/null || true
CURRENT_BRANCH=$(run_git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "master")
[ "$CURRENT_BRANCH" = "HEAD" ] && CURRENT_BRANCH="master"
echo "[deploy] Pulling latest code from GitHub ($CURRENT_BRANCH)..."
run_git pull origin "$CURRENT_BRANCH" || git -C "$PROJECT_DIR" pull origin "$CURRENT_BRANCH" || true
NEW_COMMIT=$(run_git rev-parse HEAD 2>/dev/null || echo "")

# 2b. Fast path: check if this update only modifies static webroot files or docs
CHANGED_FILES=""
if [ -n "$OLD_COMMIT" ] && [ "$OLD_COMMIT" != "$NEW_COMMIT" ]; then
    CHANGED_FILES=$(run_git diff --name-only "$OLD_COMMIT" "$NEW_COMMIT" 2>/dev/null || echo "")
fi

if [ -n "$CHANGED_FILES" ] && is_only_static "$CHANGED_FILES" && [ "${FORCE_FULL_DEPLOY:-0}" != "1" ]; then
    echo "[deploy] Only static files changed in this update:"
    echo "$CHANGED_FILES" | sed 's/^/  - /'
    echo "[deploy] Fast-path: triggering static cache refresh API on running containers..."

    SECRET_KEY=""
    if [ "$DOPPLER_AVAILABLE" = true ]; then
        SECRET_KEY=$(doppler secrets get SECRET_KEY --plain 2>/dev/null || echo "")
    elif [ -f "$PROJECT_DIR/.env" ]; then
        SECRET_KEY=$(grep -E "^SECRET_KEY=" "$PROJECT_DIR/.env" | cut -d= -f2- | tr -d '"' | tr -d "'")
    fi

    REFRESHED=false
    for URL in "http://localhost:6800/api/cache/refresh" "http://localhost:6811/api/cache/refresh" "http://localhost:6812/api/cache/refresh"; do
        STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer $SECRET_KEY" \
            -H "X-Internal-Refresh: 1" \
            -H "Host: mitch.pro" \
            -d '{"files":[]}' "$URL" || echo "000")
        if [ "$STATUS" = "200" ]; then
            REFRESHED=true
            echo "[deploy] Static cache refreshed successfully via $URL (HTTP 200)"
        fi
    done

    if [ "$REFRESHED" = true ]; then
        COUNT=$(echo "$CHANGED_FILES" | wc -l)
        echo "[deploy] Static deploy complete in seconds! ($COUNT files updated). Skipping full Docker rebuild and container swap."
        send_notification "Static deploy complete: refreshed $COUNT files in 2 seconds." "Static Deploy Successful" "low"
        exit 0
    else
        echo "[deploy] Warning: Static cache refresh API was not reachable; proceeding with full blue-green swap."
    fi
fi

echo "[deploy] Starting Blue-Green deployment swap..."

send_notification "Rebuilding and starting webserver-$INACTIVE_SLOT (Port $INACTIVE_PORT)..." "Deploy Started" "default"

# 3. Build and boot the inactive slot container, SSH gateway, conduit, LiveKit SFU, mail-rs, and tor-browser
echo "[deploy] Rebuilding and starting webserver-$INACTIVE_SLOT, ssh-gateway, conduit, livekit, mail-rs, and tor-browser..."
run_docker_compose --progress=plain up -d --build "webserver-$INACTIVE_SLOT" ssh-gateway conduit livekit mail-rs tor-browser
run_docker_compose restart conduit 2>/dev/null || true

# 4. Poll the inactive container's health check until it is fully ready
echo "[deploy] Waiting for webserver-$INACTIVE_SLOT to be fully started and responsive..."
MAX_ATTEMPTS=30
ATTEMPT=0
HEALTHY=false

while [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
    HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:$INACTIVE_PORT/enroll/" || echo "000")
    if [ "$HTTP_STATUS" = "200" ]; then
        echo "[deploy] Health check passed (HTTP 200)!"
        HEALTHY=true
        break
    fi
    echo "[deploy] Container is starting... HTTP Status: $HTTP_STATUS (Attempt $((ATTEMPT+1))/$MAX_ATTEMPTS)"
    sleep 2
    ATTEMPT=$((ATTEMPT+1))
done

if [ "$HEALTHY" = false ]; then
    echo "[deploy] Error: The new webserver-$INACTIVE_SLOT failed to become healthy. Aborting swap!"
    send_notification "Error: webserver-$INACTIVE_SLOT failed health check on port $INACTIVE_PORT. Aborting swap!" "Swap Failed" "high"
    exit 1
fi

# 5. Swap routing in the Caddyfile (targeted slot replacement, preserving headers/CSP/configs)
echo "[deploy] Swapping Caddy proxy configuration to point to webserver-$INACTIVE_SLOT..."
sed -i -E "s/(webserver-)(blue|green)(:6800)/\1$INACTIVE_SLOT\3/g" "$CADDYFILE_PATH"
if ! grep -q "webserver-$INACTIVE_SLOT:6800" "$CADDYFILE_PATH"; then
    echo "[deploy] Warning: standard sed pattern did not match; applying general replacement..."
    sed -i -E "s/webserver-[^:]+:6800/webserver-$INACTIVE_SLOT:6800/g" "$CADDYFILE_PATH"
fi

# 6. Hot-reload Caddy (0ms downtime swap)
echo "[deploy] Reloading Caddy proxy configuration..."
run_docker_compose exec -T reverse-proxy caddy reload --config /etc/caddy/Caddyfile

# 7. Absorb the first post-reload upstream connection inside the deploy. This also
# verifies the public proxy path before the old slot is removed.
echo "[deploy] Warming the newly routed application through Caddy..."
WARMED=false
for attempt in $(seq 1 10); do
    WARM_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:6800/api/health" || echo "000")
    if [ "$WARM_STATUS" = "200" ]; then
        WARMED=true
        echo "[deploy] Caddy successfully routed to webserver-$INACTIVE_SLOT (HTTP 200 via /api/health)!"
        break
    fi
    WARM_STATUS_ENROLL=$(curl -sSL --max-time 5 -o /dev/null -w "%{http_code}" -H "Host: mitchdog.com" "http://127.0.0.1:6800/enroll/" || echo "000")
    if [ "$WARM_STATUS_ENROLL" = "200" ] || [ "$WARM_STATUS_ENROLL" = "308" ] || [ "$WARM_STATUS_ENROLL" = "302" ]; then
        WARMED=true
        echo "[deploy] Caddy successfully routed to webserver-$INACTIVE_SLOT (HTTP $WARM_STATUS_ENROLL via /enroll/)!"
        break
    fi
    echo "[deploy] Waiting for Caddy routing (Attempt $attempt/10)... (/api/health: $WARM_STATUS, /enroll/: $WARM_STATUS_ENROLL)"
    sleep 2
done
if [ "$WARMED" = false ]; then
    echo "[deploy] Error: Caddy did not reach the new slot after reload. Aborting before stopping the old slot."
    exit 1
fi

# 8. Tear down the old container slot
echo "[deploy] Stopping and tearing down the old webserver-$ACTIVE_SLOT..."
run_docker_compose stop "webserver-$ACTIVE_SLOT"

if [ "$(id -u)" -eq 0 ] && [ "$REPO_OWNER" != "root" ]; then
    chown -R "$REPO_OWNER:$REPO_OWNER" "$CADDYFILE_PATH" 2>/dev/null || true
fi

echo "[deploy] Deployment successfully completed! webserver-$INACTIVE_SLOT is now serving production traffic."
send_notification "Successfully swapped traffic from webserver-$ACTIVE_SLOT to webserver-$INACTIVE_SLOT (0ms downtime)!" "Swap Successful" "high"
