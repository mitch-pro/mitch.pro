#!/usr/bin/env bash
# newsletter.sh - Wrapper script that runs newsletter.py with the secrets from
# Doppler, or from Infisical when INFISICAL=True is set in .env
# (docs/secrets-providers.md).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# >>> secrets-provider (the same block is in every script that has it; tests/test_secrets_provider.sh checks that)
# Doppler by default; INFISICAL=True in .env (or the environment) selects Infisical. The
# CLI is configured by root outside the repo (docs/secrets-providers.md). This is inline
# on purpose: these scripts run as root, and a helper file read in from the repo
# checkout would be one more place for code to be planted.
# secrets_init <.env path> | secrets_cli | secrets_available | secrets_get NAME |
# secrets_set NAME VALUE | secrets_run CMD...   (SECRETS_SUDO=1 retries with `sudo -n`)
secrets_init() {
    local env_file="${1:-}" flag="${INFISICAL:-}"
    if [ -z "$flag" ] && [ -f "$env_file" ]; then
        flag=$(grep -E '^INFISICAL=' "$env_file" | tail -n1 | cut -d= -f2- | tr -d '"' | tr -d "'" || true)
    fi
    case "$(printf '%s' "$flag" | tr '[:upper:]' '[:lower:]')" in
        true|1|yes|on) SECRETS_PROVIDER=infisical ;;
        *) SECRETS_PROVIDER=doppler ;;
    esac
    # Doppler's resolver setting, unchanged from before this file existed.
    export DOPPLER_ENABLE_DNS_RESOLVER=true
}

# The CLI's name, for callers that need it (for example under sudo).
secrets_cli() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then echo infisical; else echo doppler; fi
}

secrets_available() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        command -v infisical &> /dev/null && infisical export --format=json &> /dev/null
    else
        command -v doppler &> /dev/null && doppler secrets download --format json &> /dev/null
    fi
}

_secrets_get_once() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        "$@" infisical secrets get "$NAME_" --plain 2>/dev/null
    else
        "$@" doppler secrets get "$NAME_" --plain 2>/dev/null
    fi
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
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        command -v infisical &> /dev/null && infisical secrets set "$k=$v" &> /dev/null && return 0
        [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null && sudo -n infisical secrets set "$k=$v" &> /dev/null && return 0
    else
        command -v doppler &> /dev/null && doppler secrets set "$k=$v" &> /dev/null && return 0
        [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null && sudo -n doppler secrets set "$k=$v" &> /dev/null && return 0
    fi
    return 1
}

secrets_run() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        infisical run -- "$@"
    else
        doppler run -- "$@"
    fi
}
# <<< secrets-provider

secrets_init "$SCRIPT_DIR/.env"

if secrets_available; then
    secrets_run ./newsletter.py "$@"
    exit $?
elif command -v sudo &>/dev/null; then
    exec sudo "$(secrets_cli)" run -- ./newsletter.py "$@"
else
    exec ./newsletter.py "$@"
fi
