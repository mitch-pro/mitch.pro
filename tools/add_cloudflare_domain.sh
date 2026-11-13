#!/usr/bin/env bash
# tools/add_cloudflare_domain.sh - Helper to add a domain to the CLOUDFLARE_RECORDS secret
# Usage: ./tools/add_cloudflare_domain.sh <ZONE_ID> <DOMAIN_NAME> [A|TXT]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

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
export SECRETS_SUDO=1

if [ "$#" -lt 2 ]; then
    echo "Usage: $0 <ZONE_ID> <DOMAIN_NAME> [TYPE: A|TXT]"
    echo "Example: $0 4e92934ec0c09aaa30cbd35e62e654b5 mynewsite.com"
    echo "Example: $0 4e92934ec0c09aaa30cbd35e62e654b5 mynewsite.com TXT"
    exit 1
fi

ZONE_ID="$1"
DOMAIN_NAME="$2"
RECORD_TYPE="${3:-A}"
RECORD_TYPE=$(echo "$RECORD_TYPE" | tr '[:lower:]' '[:upper:]')

# Helper function to fetch a secret from Infisical (or .env fallback)
get_secret() {
    local key="$1"
    local val=""
    if [ -n "${!key:-}" ]; then
        echo "${!key}"
        return 0
    fi
    val=$(secrets_get "$key")
    if [ -z "$val" ] && [ -f "$BASE_DIR/.env" ]; then
        val=$(grep -E "^${key}=" "$BASE_DIR/.env" | cut -d= -f2- | tr -d '"' | tr -d "'" || echo "")
    fi
    echo "$val"
}

# Helper function to set a secret in Infisical
set_secret() {
    local key="$1"
    local val="$2"
    if secrets_set "$key" "$val"; then
        return 0
    fi
    echo "[warning] Could not update Infisical directly. Please manually set $key=\"$val\" in Infisical."
    return 1
}

# 1. Fetch Cloudflare API Token
CLOUDFLARE_API_TOKEN=$(get_secret "CLOUDFLARE_API_TOKEN")
if [ -z "$CLOUDFLARE_API_TOKEN" ]; then
    echo "[error] CLOUDFLARE_API_TOKEN not found in the secrets manager or .env."
    echo "Please add CLOUDFLARE_API_TOKEN to Infisical first."
    exit 1
fi

echo "[info] Querying Cloudflare API for '$DOMAIN_NAME' ($RECORD_TYPE record) in Zone '$ZONE_ID'..."

# 2. Fetch existing Record ID from Cloudflare
RECORD_ID=""
CF_RESPONSE=$(curl -s -X GET "https://api.cloudflare.com/client/v4/zones/$ZONE_ID/dns_records?type=$RECORD_TYPE&name=$DOMAIN_NAME" \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    -H "Content-Type: application/json")

RECORD_ID=$(echo "$CF_RESPONSE" | grep -o '"id":"[^"]*"' | head -n 1 | cut -d'"' -f4 || echo "")

# If record doesn't exist yet, auto-create it on Cloudflare
if [ -z "$RECORD_ID" ]; then
    echo "[info] Record '$DOMAIN_NAME' ($RECORD_TYPE) not found on Cloudflare. Creating initial record..."
    CURRENT_IP=$(curl -s --max-time 10 https://ipinfo.io/ip 2>/dev/null || curl -s https://ifconfig.me 2>/dev/null || echo "127.0.0.1")
    
    if [ "$RECORD_TYPE" = "TXT" ]; then
        CONTENT="v=spf1 mx ip4:$CURRENT_IP -all"
    else
        CONTENT="$CURRENT_IP"
    fi

    CREATE_RES=$(curl -s -X POST "https://api.cloudflare.com/client/v4/zones/$ZONE_ID/dns_records" \
        -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
        -H "Content-Type: application/json" \
        --data "{\"type\":\"$RECORD_TYPE\",\"name\":\"$DOMAIN_NAME\",\"content\":\"$CONTENT\",\"ttl\":120,\"proxied\":false}")

    RECORD_ID=$(echo "$CREATE_RES" | grep -o '"id":"[^"]*"' | head -n 1 | cut -d'"' -f4 || echo "")
fi

if [ -z "$RECORD_ID" ]; then
    echo "[error] Failed to retrieve or create Record ID from Cloudflare."
    echo "Response: $CF_RESPONSE"
    exit 1
fi

echo "[success] Record ID found/created: $RECORD_ID"

# 3. Format new entry
if [ "$RECORD_TYPE" = "TXT" ]; then
    NEW_ENTRY="$ZONE_ID:$RECORD_ID:TXT:$DOMAIN_NAME"
else
    NEW_ENTRY="$ZONE_ID:$RECORD_ID:$DOMAIN_NAME"
fi

# 4. Fetch existing CLOUDFLARE_RECORDS
EXISTING_RECORDS=$(get_secret "CLOUDFLARE_RECORDS")

if echo "$EXISTING_RECORDS" | grep -q "$RECORD_ID"; then
    echo "[info] Record '$RECORD_ID' ($DOMAIN_NAME) is already in CLOUDFLARE_RECORDS."
    exit 0
fi

# Append to existing records
if [ -n "$EXISTING_RECORDS" ]; then
    UPDATED_RECORDS="$EXISTING_RECORDS $NEW_ENTRY"
else
    UPDATED_RECORDS="$NEW_ENTRY"
fi

echo "[info] Updating secret CLOUDFLARE_RECORDS..."
if set_secret "CLOUDFLARE_RECORDS" "$UPDATED_RECORDS"; then
    echo "[success] Secret CLOUDFLARE_RECORDS updated successfully!"
    echo "New record added: $NEW_ENTRY"
    echo "Full records list: $UPDATED_RECORDS"
else
    echo "Updated CLOUDFLARE_RECORDS value:"
    echo "$UPDATED_RECORDS"
fi

# 5. Trigger IP sync
if [ -f "/usr/local/bin/update_ip.sh" ]; then
    echo "[info] Running IP sync..."
    /usr/local/bin/update_ip.sh --force || true
elif [ -f "$SCRIPT_DIR/update_ip.sh" ]; then
    echo "[info] Running IP sync..."
    "$SCRIPT_DIR/update_ip.sh" --force || true
fi
