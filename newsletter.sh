#!/usr/bin/env bash
# newsletter.sh - Wrapper script that runs newsletter.py with the secrets from
# Infisical (docs/secrets-providers.md).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

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

if secrets_available; then
    secrets_run ./newsletter.py "$@"
    exit $?
elif command -v sudo &>/dev/null; then
    exec sudo infisical run -- ./newsletter.py "$@"
else
    exec ./newsletter.py "$@"
fi
