#!/usr/bin/env bash
# tools/update_ip.sh - Dynamic IP Watcher & DNS Auto-Updater
# Updates GitHub Secrets (SSH_HOST), Cloudflare DNS (A & TXT/SPF), and FreeDNS when your public IP changes.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -d "/home/mitch/server/bun" ]; then
    BASE_DIR="/home/mitch/server/bun"
elif [ -d "/home/mitch/bun-server-main/bun-server" ]; then
    BASE_DIR="/home/mitch/bun-server-main/bun-server"
elif [ -f "$SCRIPT_DIR/../docker-compose.yml" ]; then
    BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
elif [ -f "$SCRIPT_DIR/docker-compose.yml" ]; then
    BASE_DIR="$SCRIPT_DIR"
else
    BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
fi

IP_CACHE_FILE="$BASE_DIR/.current_public_ip"
TEMP_IP_FILE="$BASE_DIR/.temp_public_ip_override"

# >>> secrets-provider (the same block is in every script that has it; tests/test_secrets_provider.sh checks that)
# Infisical. The CLI is configured by root outside the repo (docs/secrets-providers.md). This
# is inline on purpose: these scripts run as root, and a helper file read in from the repo
# checkout would be one more place for code to be planted.
# secrets_available | secrets_get NAME | secrets_set NAME VALUE | secrets_run CMD...
# (SECRETS_SUDO=1 retries with `sudo -n`)
secrets_available() {
    command -v infisical &> /dev/null && infisical export --format=json &> /dev/null
}

_secrets_get_once() {
    "$@" infisical secrets get "$NAME_" --plain 2>/dev/null
}

secrets_get() {
    local NAME_="$1" val=""
    val=$(_secrets_get_once || echo "")
    if [ -z "$val" ] && [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null; then
        val=$(_secrets_get_once sudo -n || echo "")
    fi
    printf '%s' "$val"
}

secrets_set() {
    local k="$1" v="$2"
    command -v infisical &> /dev/null && infisical secrets set "$k=$v" &> /dev/null && return 0
    [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null && sudo -n infisical secrets set "$k=$v" &> /dev/null && return 0
    return 1
}

secrets_run() {
    infisical run -- "$@"
}
# <<< secrets-provider
# Helper function to fetch a secret from Infisical (or .env fallback), matching deploy.sh pattern
get_secret() {
    local key="$1"
    local val=""

    # 1. Check if variable is already set/exported in environment (e.g. from `infisical run`)
    if [ -n "${!key:-}" ]; then
        echo "${!key}"
        return 0
    fi

    # 2. Try fetching from the secrets CLI (user or sudo context in BASE_DIR)
    val=$(cd "$BASE_DIR" 2>/dev/null && SECRETS_SUDO=1 secrets_get "$key" || echo "")
    if [ -n "$val" ]; then
        echo "$val"
        return 0
    fi

    # 4. Fallback to .env file
    for env_path in "$BASE_DIR/.env" "/home/mitch/server/bun/.env"; do
        if [ -f "$env_path" ]; then
            val=$(grep -E "^${key}=" "$env_path" | cut -d= -f2- | tr -d '"' | tr -d "'" || echo "")
            if [ -n "$val" ]; then
                echo "$val"
                return 0
            fi
        fi
    done

    echo "$val"
}

FORCE_UPDATE=false
TEMP_MODE=false
TEMP_ARG=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --force)
            FORCE_UPDATE=true
            shift
            ;;
        --temp)
            TEMP_MODE=true
            TEMP_ARG="${2:-}"
            shift 2 || shift
            ;;
        *)
            shift
            ;;
    esac
done

# Handle --temp stop / clear or setting new temp IP
if [ "$TEMP_MODE" = true ]; then
    CLEAN_TEMP=$(echo "$TEMP_ARG" | tr '[:upper:]' '[:lower:]')
    if [ "$CLEAN_TEMP" = "stop" ] || [ "$CLEAN_TEMP" = "clear" ] || [ "$CLEAN_TEMP" = "off" ] || [ "$CLEAN_TEMP" = "disable" ]; then
        if [ -f "$TEMP_IP_FILE" ]; then
            rm -f "$TEMP_IP_FILE"
            echo "[$(date -Iseconds)] [ip-watcher] Temporary IP override stopped/cleared."
        else
            echo "[$(date -Iseconds)] [ip-watcher] No active temporary IP override found."
        fi
        FORCE_UPDATE=true
    elif [ -n "$TEMP_ARG" ]; then
        echo "$TEMP_ARG" > "$TEMP_IP_FILE"
        echo "[$(date -Iseconds)] [ip-watcher] Temporary IP override set to '$TEMP_ARG'."
        FORCE_UPDATE=true
    else
        echo "Usage: $0 --temp <IP_ADDRESS> | $0 --temp stop"
        exit 1
    fi
fi

# 1. Fetch current or temporary IP address
IS_TEMP=false
if [ -f "$TEMP_IP_FILE" ]; then
    NEW_IP=$(cat "$TEMP_IP_FILE" | tr -d ' \n\r')
    IS_TEMP=true
else
    NEW_IP=$(curl -s --max-time 10 https://ipinfo.io/ip 2>/dev/null || curl -s --max-time 10 https://ifconfig.me 2>/dev/null || echo "")
    NEW_IP=$(echo "$NEW_IP" | tr -d ' \n\r')
fi

if [ -z "$NEW_IP" ] || ! echo "$NEW_IP" | grep -Eq '^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$'; then
    echo "[$(date -Iseconds)] [ip-watcher] ERROR: Unable to determine valid public IP (got: '${NEW_IP:-empty}')."
    exit 1
fi

OLD_IP=""
if [ -f "$IP_CACHE_FILE" ]; then
    OLD_IP=$(cat "$IP_CACHE_FILE" | tr -d ' \n\r')
elif [ -f "/usr/local/.current_public_ip" ]; then
    OLD_IP=$(cat "/usr/local/.current_public_ip" | tr -d ' \n\r')
fi

# Check current DNS record for mitch.pro
DNS_IP=$(dig +short mitch.pro @1.1.1.1 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1 || echo "")
if [ -z "$DNS_IP" ]; then
    DNS_IP=$(dig +short mitch.pro 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1 || echo "")
fi

NEED_UPDATE=false
if [ "$FORCE_UPDATE" = true ]; then
    NEED_UPDATE=true
elif [ "$NEW_IP" != "$OLD_IP" ]; then
    NEED_UPDATE=true
elif [ -n "$DNS_IP" ] && [ "$NEW_IP" != "$DNS_IP" ]; then
    NEED_UPDATE=true
fi

if [ "$NEED_UPDATE" = false ]; then
    # IP has not changed and DNS is in sync, nothing to do
    exit 0
fi

if [ "$IS_TEMP" = true ]; then
    echo "[$(date -Iseconds)] [ip-watcher] TEMP OVERRIDE ACTIVE: '${OLD_IP:-none}' -> '$NEW_IP' (DNS: '${DNS_IP:-unknown}')"
elif [ "$FORCE_UPDATE" = true ]; then
    echo "[$(date -Iseconds)] [ip-watcher] Force update triggered: IP is '$NEW_IP', cached is '${OLD_IP:-none}', DNS is '${DNS_IP:-unknown}'"
elif [ "$NEW_IP" != "$OLD_IP" ]; then
    echo "[$(date -Iseconds)] [ip-watcher] IP change detected: '${OLD_IP:-none}' -> '$NEW_IP' (DNS: '${DNS_IP:-unknown}')"
elif [ -n "$DNS_IP" ] && [ "$NEW_IP" != "$DNS_IP" ]; then
    echo "[$(date -Iseconds)] [ip-watcher] DNS mismatch detected: DNS 'mitch.pro' is '$DNS_IP', current IP is '$NEW_IP'. Triggering update..."
fi

# 2. Update GitHub Secret (SSH_HOST)
if command -v gh &>/dev/null; then
    echo "[ip-watcher] Updating GitHub secret SSH_HOST to $NEW_IP..."
    if gh secret set SSH_HOST --repo Mitch-Fogler/bun-server --body "$NEW_IP" 2>/dev/null; then
        echo "[ip-watcher] GitHub secret SSH_HOST updated successfully."
    else
        echo "[ip-watcher] WARNING: Failed to update GitHub secret via gh CLI."
    fi
fi

# 3. Fetch secrets from the secrets manager (or .env)
CLOUDFLARE_API_TOKEN=$(get_secret "CLOUDFLARE_API_TOKEN")
CLOUDFLARE_RECORDS=$(get_secret "CLOUDFLARE_RECORDS")
FREEDNS_UPDATE_URLS=$(get_secret "FREEDNS_UPDATE_URLS")

# 4. Update Cloudflare DNS Records (supports both A records and TXT/SPF records)
if [ -n "$CLOUDFLARE_API_TOKEN" ] && [ -n "$CLOUDFLARE_RECORDS" ]; then
    echo "[ip-watcher] Updating Cloudflare records..."
    for item in $CLOUDFLARE_RECORDS; do
        IFS=':' read -r ZONE_ID RECORD_ID FIELD3 FIELD4 <<< "$item"
        
        RECORD_TYPE="A"
        DOMAIN_NAME=""
        
        if [ -n "$FIELD4" ]; then
            RECORD_TYPE=$(echo "$FIELD3" | tr '[:lower:]' '[:upper:]')
            DOMAIN_NAME="$FIELD4"
        else
            DOMAIN_NAME="$FIELD3"
        fi

        if [ "$RECORD_TYPE" = "TXT" ]; then
            RECORD_CONTENT="v=spf1 mx ip4:$NEW_IP -all"
            PROXIED="false"
        else
            RECORD_TYPE="A"
            RECORD_CONTENT="$NEW_IP"
            PROXIED="false"
        fi

        echo "[ip-watcher] Updating Cloudflare $RECORD_TYPE record for $DOMAIN_NAME..."
        CF_RESP=$(curl -s -X PUT "https://api.cloudflare.com/client/v4/zones/$ZONE_ID/dns_records/$RECORD_ID" \
             -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
             -H "Content-Type: application/json" \
             --data "{\"type\":\"$RECORD_TYPE\",\"name\":\"$DOMAIN_NAME\",\"content\":\"$RECORD_CONTENT\",\"ttl\":120,\"proxied\":$PROXIED}" 2>/dev/null || echo "")
        if echo "$CF_RESP" | grep -q '"success":true'; then
            echo "[ip-watcher] Cloudflare $RECORD_TYPE record for $DOMAIN_NAME updated successfully."
        else
            echo "[ip-watcher] ERROR: Cloudflare update failed for $DOMAIN_NAME. Response: $CF_RESP"
        fi
    done
else
    echo "[ip-watcher] WARNING: CLOUDFLARE_API_TOKEN or CLOUDFLARE_RECORDS not found. Skipping Cloudflare DNS update."
fi

# 5. Update FreeDNS (afraid.org) Records (if FREEDNS_UPDATE_URLS is provided)
if [ -n "$FREEDNS_UPDATE_URLS" ]; then
    echo "[ip-watcher] Updating FreeDNS records..."
    for url in $FREEDNS_UPDATE_URLS; do
        curl -s "$url" > /dev/null || true
    done
fi

# Save the new IP to cache file
echo "$NEW_IP" > "$IP_CACHE_FILE"
if [ "$BASE_DIR" != "/usr/local" ] && [ -w "/usr/local" ]; then
    echo "$NEW_IP" > "/usr/local/.current_public_ip" 2>/dev/null || true
fi
echo "[$(date -Iseconds)] [ip-watcher] Successfully updated public IP cache to $NEW_IP"
